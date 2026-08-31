//! Chat broker HTTP: provider listing, owner-scoped sessions, send, events,
//! cancel, retire. Credentials and provider base URLs never appear here.

use super::api::{body_str, body_u64, parse_limit, principal_str, Fail, RouteResult};
use super::chat::{ChatFail, ChatHandle, ProviderStatus, SessionView};
use super::http::{Conn, Head, Method, Resp};
use super::json::{obj, s, Value};
use super::routes::{call_state, is_read, method_not_allowed, require_cap, secret_of, Outcome, RouteCtx};
use super::routes_control::read_json_body;
use super::util::now_ms;
use makepad_asset_chat::context::ClientProfile;
use makepad_asset_chat::session::SessionId;
use makepad_asset_chat::wire::{
    AttachmentBinding, ChatEvent, ProviderKind, ToolOutcome, MAX_ATTACHMENTS,
};
use crate::{validate_namespace, Capability, PrincipalId};
use makepad_asset_data::AssetRevisionId;
use std::str::FromStr;

const EVENT_WAIT_SLICE_MS: u64 = 250;
const MAX_CHAT_EVENT_WAITERS: usize = 8;

struct WaiterSlot<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> WaiterSlot<'a> {
    fn acquire(counter: &'a std::sync::atomic::AtomicUsize) -> Option<WaiterSlot<'a>> {
        let mut current = counter.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            if current >= MAX_CHAT_EVENT_WAITERS {
                return None;
            }
            match counter.compare_exchange(
                current,
                current + 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return Some(WaiterSlot(counter)),
                Err(found) => current = found,
            }
        }
    }
}

impl<'a> Drop for WaiterSlot<'a> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub fn dispatch(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    segs: &[&str],
) -> Option<RouteResult<Outcome>> {
    let m = head.method;
    let result = match segs {
        ["v1", "chat", "providers"] => {
            if is_read(m) {
                providers(head, rc)
            } else {
                method_not_allowed()
            }
        }
        ["v1", "chat", "sessions"] if is_read(m) => sessions_list(head, rc),
        ["v1", "chat", "sessions"] if m == Method::Post => session_create(conn, head, rc),
        ["v1", "chat", "sessions", id] if is_read(m) => session_get(head, rc, id),
        ["v1", "chat", "sessions", id] if m == Method::Delete => session_retire(head, rc, id),
        ["v1", "chat", "sessions", id, "send"] if m == Method::Post => {
            session_send(conn, head, rc, id)
        }
        ["v1", "chat", "sessions", id, "events"] if is_read(m) => session_events(head, rc, id),
        ["v1", "chat", "sessions", id, "transcript"] if is_read(m) => {
            session_transcript(head, rc, id)
        }
        ["v1", "chat", "sessions", id, "cancel"] if m == Method::Post => {
            session_cancel(conn, head, rc, id)
        }
        ["v1", "chat", "sessions", id, "tool-result"] if m == Method::Post => {
            session_tool_result(conn, head, rc, id)
        }
        _ => return None,
    };
    Some(result)
}

fn chat_of(rc: &RouteCtx) -> RouteResult<&ChatHandle> {
    rc.chat.as_ref().ok_or(Fail::StateDown)
}

fn sid_of(id: &str) -> RouteResult<SessionId> {
    SessionId::parse(id).ok_or(Fail::Http(400, "malformed session id"))
}

fn auth_owner(head: &Head, rc: &RouteCtx, ns: Option<&str>) -> RouteResult<(PrincipalId, String)> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let ns_owned = ns.map(str::to_string);
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        if let Some(ns) = &ns_owned {
            require_cap(ctx, &p, Capability::Chat, ns)?;
        }
        Ok((p, secret))
    })
}

fn check_known_fields(v: &Value, allowed: &[&str], what: &'static str) -> RouteResult<()> {
    if let Value::Obj(pairs) = v {
        for (key, _) in pairs {
            if !allowed.contains(&key.as_str()) {
                return Err(Fail::Http(400, what));
            }
        }
    }
    Ok(())
}

fn providers(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let _ = auth_owner(head, rc, None)?;
    let rows = match chat_of(rc)?.list_providers() {
        Ok(rows) => rows,
        Err(e) => return Ok(chat_outcome(e)),
    };
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("providers", Value::Arr(rows.iter().map(provider_value).collect()))]),
    )))
}

fn session_create(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    check_known_fields(
        &body,
        &["api_version", "namespace", "provider", "client", "client_key", "context_key"],
        "unknown chat session field",
    )?;
    match body_u64(&body, "api_version") {
        Some(1) => {}
        _ => return Err(Fail::Http(400, "unsupported api_version")),
    }
    let ns = body_str(&body, "namespace")?.to_string();
    validate_namespace(&ns).map_err(Fail::Srv)?;
    let provider = ProviderKind::from_slug(body_str(&body, "provider")?)
        .ok_or(Fail::Http(400, "unknown chat provider"))?;
    // The connecting app's declared profile selects its taught context and
    // tool surface. Absent = general; unknown slugs are refused.
    let profile = match body.get("client") {
        None => ClientProfile::General,
        Some(v) => v
            .as_str()
            .and_then(ClientProfile::from_slug)
            .ok_or(Fail::Http(400, "unknown chat client profile"))?,
    };
    // Durable identity: both keys make this a create-or-resume of ONE
    // conversation per (principal, client, game). One key alone is a
    // client bug, refused rather than half-honoured.
    let keys = match (chat_key_of(&body, "client_key")?, chat_key_of(&body, "context_key")?) {
        (Some(client_key), Some(context_key)) => Some((client_key, context_key)),
        (None, None) => None,
        _ => return Err(Fail::Http(400, "client_key and context_key go together")),
    };
    let (owner, token) = auth_owner(head, rc, Some(&ns))?;
    // Resuming/rebinding a conversation that currently lives under ANOTHER
    // namespace additionally requires Chat on that ORIGINAL namespace —
    // losing a namespace must mean losing its conversations.
    let authorize_ns = |original: &str| require_chat_ns(head, rc, original).is_ok();
    let (view, resumed) =
        match chat_of(rc)?.create(owner, ns, token, provider, profile, keys, &authorize_ns) {
            Ok(v) => v,
            Err(e) => return Ok(chat_outcome(e)),
        };
    // 201 = a fresh session; 200 = the existing conversation came back.
    let status = if resumed { 200 } else { 201 };
    Ok(Outcome::Resp(Resp::json(status, &session_value(&view))))
}

/// An optional session key: absent/null is none; present, it must have the
/// shape the client crate enforces (`wire::chat_key_ok`), so a key can
/// never spell a path or carry a control byte to a screen.
fn chat_key_of(body: &Value, key: &'static str) -> RouteResult<Option<String>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or(Fail::Http(400, "malformed chat key"))?;
            if !makepad_asset_client::wire::chat_key_ok(s) {
                return Err(Fail::Http(400, "malformed chat key"));
            }
            Ok(Some(s.to_string()))
        }
    }
}

/// The durable conversation as the client renders it: the LAST rows that
/// fit the broker's budget (`TRANSCRIPT_MAX_ROWS` rows,
/// `TRANSCRIPT_MAX_TEXT_BYTES` of text, each row clipped to
/// `TRANSCRIPT_MAX_ROW_BYTES`), thinking stripped, one `tool` row per
/// executed tool.
fn session_transcript(head: &Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let (owner, _) = auth_owner(head, rc, None)?;
    // Ownership alone is not enough: like send/cancel/delete, reading the
    // conversation requires the Chat capability on the session's
    // namespace — an owner that lost the namespace lost its transcripts.
    let session = match chat_of(rc)?.get(owner, id.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    require_chat_ns(head, rc, &session.namespace)?;
    let view = match chat_of(rc)?.transcript(owner, id) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    let rows: Vec<Value> = view
        .rows
        .iter()
        .map(|row| {
            let mut pairs = vec![("role", s(row.role.slug())), ("text", s(row.text.clone()))];
            if let Some(tool) = &row.tool {
                pairs.push(("tool", s(tool.clone())));
            }
            if let Some(outcome) = row.outcome {
                pairs.push(("outcome", s(outcome)));
            }
            obj(pairs)
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("session", s(view.id.as_str())),
            ("provider", s(view.provider.slug())),
            ("turn", Value::Int(view.turn.min(i64::MAX as u64) as i64)),
            ("truncated", Value::Bool(view.truncated)),
            ("messages", Value::Arr(rows)),
        ]),
    )))
}

/// The connected app's answer to a client-executed tool call (the game's
/// world tools). Owner-scoped like send/cancel; the outcome parses through
/// the wire type's own bounds.
fn session_tool_result(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    id: &str,
) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    check_known_fields(&body, &["id", "outcome"], "unknown tool result field")?;
    let call_id = body_str(&body, "id")?;
    if call_id.is_empty() || call_id.len() > 64 {
        return Err(Fail::Http(400, "malformed tool call id"));
    }
    let call_id = call_id.to_string();
    let outcome = {
        let raw = body.get("outcome").ok_or(Fail::Http(400, "missing outcome"))?;
        ToolOutcome::decode(&to_wire_value(raw))
            .map_err(|_| Fail::Http(400, "malformed tool outcome"))?
    };
    let (owner, _) = auth_owner(head, rc, None)?;
    let view = match chat_of(rc)?.get(owner, id.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    require_chat_ns(head, rc, &view.namespace)?;
    match chat_of(rc)?.tool_result(owner, id, call_id, outcome) {
        Ok(()) => Ok(Outcome::Resp(Resp::json(200, &obj(vec![("accepted", Value::Bool(true))])))),
        Err(e) => Ok(chat_outcome(e)),
    }
}

/// Every live session this principal owns — how an observer finds a play
/// session's transcript (GET the ids here, then each session's /events).
fn sessions_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let (owner, _) = auth_owner(head, rc, None)?;
    let views = match chat_of(rc)?.list(owner) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    let rows: Vec<Value> = views.iter().map(session_value).collect();
    Ok(Outcome::Resp(Resp::json(200, &obj(vec![("sessions", Value::Arr(rows))]))))
}

fn session_get(head: &Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let (owner, _) = auth_owner(head, rc, None)?;
    let view = match chat_of(rc)?.get(owner, id) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    Ok(Outcome::Resp(Resp::json(200, &session_value(&view))))
}

fn session_send(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    check_known_fields(
        &body,
        &["text", "attachments", "dynamic_context"],
        "unknown chat send field",
    )?;
    let text = body_str(&body, "text")?.to_string();
    let dynamic_context = match body.get("dynamic_context") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let text = value.as_str().ok_or(Fail::Http(400, "malformed dynamic context"))?;
            if text.is_empty() || text.len() > 4096 || text.contains('\0') {
                return Err(Fail::Http(400, "malformed dynamic context"));
            }
            Some(text.to_string())
        }
    };
    let attachments = attachments_of(&body)?;
    let (owner, _) = auth_owner(head, rc, None)?;
    let view = match chat_of(rc)?.get(owner, id.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    require_chat_ns(head, rc, &view.namespace)?;
    let turn = match chat_of(rc)?.send(owner, id, text, attachments, dynamic_context) {
        Ok(t) => t,
        Err(e) => return Ok(chat_outcome(e)),
    };
    Ok(Outcome::Resp(Resp::json(200, &obj(vec![("turn", Value::Int(turn.min(i64::MAX as u64) as i64))]))))
}

fn session_events(head: &Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let (owner, _) = auth_owner(head, rc, None)?;
    let after = match head.query_get("after") {
        None => 0,
        Some(t) => {
            if t.is_empty() || t.len() > 12 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Fail::Http(400, "malformed after"));
            }
            t.parse().map_err(|_| Fail::Http(400, "malformed after"))?
        }
    };
    let wait_ms = match head.query_get("wait") {
        None => 0,
        Some(t) => {
            if t.is_empty() || t.len() > 6 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Fail::Http(400, "malformed wait"));
            }
            let n: u64 = t.parse().map_err(|_| Fail::Http(400, "malformed wait"))?;
            n.min(rc.cfg.chat.event_max_wait_ms)
        }
    };
    let limit = parse_limit(head.query_get("limit"), 64, 256)? as u32;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut slot: Option<WaiterSlot<'_>> = None;
    loop {
        let (events, cursor) = match chat_of(rc)?.events(owner, id.clone(), after, limit) {
            Ok(v) => v,
            Err(e) => return Ok(chat_outcome(e)),
        };
        let park = events.is_empty()
            && std::time::Instant::now() < deadline
            && match &slot {
                Some(_) => true,
                None => {
                    slot = WaiterSlot::acquire(&rc.chat_event_waiters);
                    slot.is_some()
                }
            };
        if !park {
            let encoded: Vec<Value> = events.iter().map(event_value).collect();
            return Ok(Outcome::Resp(Resp::json(
                200,
                &obj(vec![
                    ("events", Value::Arr(encoded)),
                    ("cursor", Value::Int(cursor.min(i64::MAX as u64) as i64)),
                ]),
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(
            EVENT_WAIT_SLICE_MS.min(wait_ms.max(1)),
        ));
    }
}

fn session_cancel(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let _body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let (owner, _) = auth_owner(head, rc, None)?;
    let view = match chat_of(rc)?.get(owner, id.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    require_chat_ns(head, rc, &view.namespace)?;
    let view = match chat_of(rc)?.cancel(owner, id) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    Ok(Outcome::Resp(Resp::json(200, &session_value(&view))))
}

fn session_retire(head: &Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    let id = sid_of(id)?;
    let (owner, _) = auth_owner(head, rc, None)?;
    let view = match chat_of(rc)?.get(owner, id.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    require_chat_ns(head, rc, &view.namespace)?;
    let retired = match chat_of(rc)?.retire(owner, id) {
        Ok(v) => v,
        Err(e) => return Ok(chat_outcome(e)),
    };
    Ok(Outcome::Resp(Resp::json(200, &obj(vec![("retired", Value::Bool(retired))]))))
}

fn require_chat_ns(head: &Head, rc: &RouteCtx, ns: &str) -> RouteResult<()> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let ns = ns.to_string();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::Chat, &ns)
    })
}

fn attachments_of(body: &Value) -> RouteResult<Vec<AttachmentBinding>> {
    let Some(raw) = body.get("attachments") else {
        return Ok(Vec::new());
    };
    let arr = raw.as_arr().ok_or(Fail::Http(400, "attachments must be an array"))?;
    if arr.len() > MAX_ATTACHMENTS {
        return Err(Fail::Http(400, "too many attachments"));
    }
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        check_known_fields(entry, &["revision", "role"], "unknown attachment field")?;
        let rev = body_str(entry, "revision")?;
        let revision =
            AssetRevisionId::from_str(rev).map_err(|_| Fail::Http(400, "malformed revision id"))?;
        let role = body_str(entry, "role")?.to_string();
        let binding = AttachmentBinding { revision, role };
        binding.validate().map_err(|_| Fail::Http(400, "malformed attachment"))?;
        out.push(binding);
    }
    Ok(out)
}

fn provider_value(row: &ProviderStatus) -> Value {
    let mut pairs = vec![("kind", s(row.kind.slug())), ("locality", s(row.locality.slug()))];
    if row.available {
        pairs.push(("state", s("available")));
        if let Some(model) = &row.model {
            pairs.push(("model", s(model.clone())));
        }
    } else {
        pairs.push(("state", s("unavailable")));
        if let Some(reason) = &row.reason {
            pairs.push(("reason", s(reason.clone())));
        }
    }
    obj(pairs)
}

fn session_value(view: &SessionView) -> Value {
    let mut pairs = vec![
        ("session", s(view.id.as_str())),
        ("namespace", s(view.namespace.clone())),
        ("provider", s(view.provider.slug())),
        ("owner", s(principal_str(&view.owner))),
        ("state", s(view.state)),
        ("turn", Value::Int(view.turn.min(i64::MAX as u64) as i64)),
        ("idle", Value::Bool(view.idle)),
    ];
    // Keyed sessions echo their durable identity; ephemeral ones omit it.
    if let Some(key) = &view.client_key {
        pairs.push(("client_key", s(key.clone())));
    }
    if let Some(key) = &view.context_key {
        pairs.push(("context_key", s(key.clone())));
    }
    obj(pairs)
}

/// STRUCTURAL conversion between the chat wire's value and the host's —
/// never through text. The host parser is stricter than the wire (it
/// refuses floats, duplicate keys and deep nesting), so a text roundtrip
/// turned legitimate events — a tool call with a float argument — into
/// "event encode failed" fallbacks on the wire (observed live in the
/// village loop).
fn to_host_value(v: &makepad_asset_client::json::Value) -> Value {
    use makepad_asset_client::json::Value as C;
    match v {
        C::Null => Value::Null,
        C::Bool(b) => Value::Bool(*b),
        C::Int(i) => Value::Int(*i),
        C::F64(f) => Value::F64(*f),
        C::Str(text) => Value::Str(text.clone()),
        C::Arr(items) => Value::Arr(items.iter().map(to_host_value).collect()),
        C::Obj(pairs) => Value::Obj(
            pairs.iter().map(|(k, v)| (k.clone(), to_host_value(v))).collect(),
        ),
    }
}

fn to_wire_value(v: &Value) -> makepad_asset_client::json::Value {
    use makepad_asset_client::json::Value as C;
    match v {
        Value::Null => C::Null,
        Value::Bool(b) => C::Bool(*b),
        Value::Int(i) => C::Int(*i),
        Value::F64(f) => C::F64(*f),
        Value::Str(text) => C::Str(text.clone()),
        Value::Arr(items) => C::Arr(items.iter().map(to_wire_value).collect()),
        Value::Obj(pairs) => C::Obj(
            pairs.iter().map(|(k, v)| (k.clone(), to_wire_value(v))).collect(),
        ),
    }
}

fn event_value(ev: &ChatEvent) -> Value {
    to_host_value(&ev.encode())
}

fn chat_outcome(e: ChatFail) -> Outcome {
    match e {
        ChatFail::Down => Outcome::Resp(Resp::error(503, "state unavailable")),
        ChatFail::NotFound => Outcome::Resp(Resp::error(404, "not found")),
        ChatFail::Forbidden => Outcome::Resp(Resp::error(403, "forbidden")),
        ChatFail::Persist { message } => Outcome::Resp(Resp::json(
            500,
            &obj(vec![("error", s("persistence")), ("message", s(message))]),
        )),
        ChatFail::Busy => Outcome::Resp(Resp::json(
            409,
            &obj(vec![("error", s("busy"))]),
        )),
        ChatFail::Sealed { reason } => Outcome::Resp(Resp::json(
            409,
            &obj(vec![("error", s("sealed")), ("reason", s(reason))]),
        )),
        ChatFail::ProviderUnavailable { reason } => Outcome::Resp(Resp::json(
            503,
            &obj(vec![("error", s("provider_unavailable")), ("reason", s(reason))]),
        )),
        ChatFail::ProviderError { message } => Outcome::Resp(Resp::json(
            502,
            &obj(vec![("error", s("provider")), ("message", s(message))]),
        )),
        ChatFail::TooLarge { what } => Outcome::Resp(Resp::json(
            413,
            &obj(vec![("error", s("too_large")), ("what", s(what))]),
        )),
        ChatFail::TooMany { what } => Outcome::Resp(Resp::json(
            400,
            &obj(vec![("error", s("too_many")), ("what", s(what))]),
        )),
        ChatFail::HistoryFull => Outcome::Resp(Resp::json(
            409,
            &obj(vec![("error", s("history_full"))]),
        )),
        ChatFail::InvalidAttachment { what } => Outcome::Resp(Resp::json(
            400,
            &obj(vec![("error", s("invalid_attachment")), ("what", s(what))]),
        )),
        ChatFail::OverBudget { what } => Outcome::Resp(Resp::json(
            413,
            &obj(vec![("error", s("over_budget")), ("what", s(what))]),
        )),
        ChatFail::ToolsConnect { message } => Outcome::Resp(Resp::json(
            503,
            &obj(vec![("error", s("tools_unavailable")), ("message", s(message))]),
        )),
        ChatFail::NoClientTool { what } => Outcome::Resp(Resp::json(
            409,
            &obj(vec![("error", s("no_client_tool")), ("what", s(what))]),
        )),
    }
}
