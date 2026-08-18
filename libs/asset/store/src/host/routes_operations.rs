//! Typed asset-operation routes: the versioned operation registry, owner-
//! scoped operation creation/status/events/cancel/retry, and the worker
//! finalize route.
//!
//! Capability map (extending the control-plane map):
//!   operation create/cancel/retry ..... `operation_run` on the operation ns
//!   operation get/events .............. any authenticated principal, but
//!       rows are OWNER-scoped: a foreign operation reads as 404
//!   operation-types listing ........... any authenticated principal
//!   finalize .......................... `job_worker` on the operation ns
//!
//! The create body is parsed FAIL-CLOSED: an unknown api_version, an unknown
//! top-level field, an unknown input field, or an unknown parameter refuses
//! with a 400 instead of being ignored. This surface is the one an LLM
//! gateway drives; silently dropped fields would turn model mistakes into
//! silently different operations.

use super::api::{
    body_str, body_u64, media_str, operation_str, parse_job, parse_media, parse_operation,
    parse_role, parse_tier, principal_str, role_str, tier_str, Fail, RouteResult,
};
use super::events::{self, EventBody};
use super::http::{Conn, Head, Resp};
use super::json::{obj, s, Value};
use super::routes::{call_state, require_cap, secret_of, Outcome, RouteCtx};
use super::routes_control::{read_json_body, worker_name, worker_suffix};
use super::state::{envelope_build, StateCtx, MAX_JOB_NAMESPACES};
use super::util::{now_ms, rand16, to_hex};
use crate::operations::{OperationOutputFacts, ParamValue};
use crate::{
    AliasExpect, ArmedJob, Capability, NewJob, OperationCreateOutcome, OperationCreateRequest,
    OperationId, OperationInputBinding, OperationPublication, OperationResultFacts,
    OperationSnapshot, PrincipalId, ServerError,
};
use makepad_asset_data::{
    AssetAlias, AssetFile, AssetId, AssetRevisionId, Bounds, DeviceTier, ImageDims, Metrics,
    ThumbnailMedia, ThumbnailMeta, Vec3,
};
use std::str::FromStr;

/// Operation executor jobs retry through operation.retry, not the job-level
/// attempt loop: one attempt per round keeps "which round produced this" an
/// exact statement.
const OPERATION_JOB_ATTEMPTS: u32 = 1;

/// Long-poll slice for the operation event wait loop. Each slice is one
/// bounded state-thread call, so a parked waiter never pins the state.
const EVENT_WAIT_SLICE_MS: u64 = 250;

/// Most operation-event long-polls parked at once. A parked waiter holds a
/// control-plane connection thread for its whole wait, and the plane has a
/// hard connection cap — without this budget, one owner opening waiters
/// could starve worker heartbeats/claims/finalize into lease expiry. Over
/// budget answers immediately with the empty page (the client simply polls
/// again); it never parks.
const MAX_OP_EVENT_WAITERS: usize = 16;

/// RAII slot in the waiter budget.
struct WaiterSlot<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> WaiterSlot<'a> {
    fn acquire(counter: &'a std::sync::atomic::AtomicUsize) -> Option<WaiterSlot<'a>> {
        let mut current = counter.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            if current >= MAX_OP_EVENT_WAITERS {
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

pub fn oid_of(t: &str) -> RouteResult<OperationId> {
    parse_operation(t).ok_or(Fail::Http(400, "malformed operation id"))
}

// ---------------------------------------------------------------------------
// registry listing
// ---------------------------------------------------------------------------

pub fn operation_types(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let caps = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core.operations().capabilities(now)
    })?;
    let types: Vec<Value> = caps
        .iter()
        .map(|c| {
            let def = c.def;
            let inputs: Vec<Value> = def
                .inputs
                .iter()
                .map(|slot| {
                    obj(vec![
                        ("name", s(slot.name)),
                        ("min_count", Value::Int(slot.min_count as i64)),
                        ("max_count", Value::Int(slot.max_count as i64)),
                        (
                            "kinds",
                            Value::Arr(
                                slot.accepted_kinds
                                    .iter()
                                    .map(|k| s(super::api::kind_str(*k)))
                                    .collect(),
                            ),
                        ),
                        (
                            "roles",
                            Value::Arr(
                                slot.accepted_roles.iter().map(|r| s(role_str(*r))).collect(),
                            ),
                        ),
                        (
                            "media",
                            Value::Arr(
                                slot.accepted_media.iter().map(|m| s(media_str(*m))).collect(),
                            ),
                        ),
                    ])
                })
                .collect();
            let params: Vec<Value> = def
                .params
                .iter()
                .map(|p| {
                    use crate::operations::ParamSpecKind;
                    let mut pairs = vec![("name", s(p.name))];
                    match p.kind {
                        ParamSpecKind::Int { min, max, default } => {
                            pairs.push(("type", s("integer")));
                            pairs.push(("min", Value::Int(min)));
                            pairs.push(("max", Value::Int(max)));
                            pairs.push(("default", Value::Int(default)));
                        }
                        ParamSpecKind::Text { max_bytes, default } => {
                            pairs.push(("type", s("text")));
                            pairs.push(("max_bytes", Value::Int(max_bytes as i64)));
                            pairs.push(("default", s(default)));
                        }
                        ParamSpecKind::Bool { default } => {
                            pairs.push(("type", s("boolean")));
                            pairs.push(("default", Value::Bool(default)));
                        }
                    }
                    obj(pairs)
                })
                .collect();
            let outputs: Vec<Value> = def
                .outputs
                .iter()
                .map(|o| {
                    obj(vec![
                        ("name", s(o.name)),
                        ("kind", s(super::api::kind_str(o.kind))),
                        (
                            "required_roles",
                            Value::Arr(o.required_roles.iter().map(|r| s(role_str(*r))).collect()),
                        ),
                    ])
                })
                .collect();
            obj(vec![
                ("kind", s(def.kind)),
                ("revision", Value::Int(def.revision as i64)),
                ("label", s(def.label)),
                ("description", s(def.description)),
                ("inputs", Value::Arr(inputs)),
                ("params", Value::Arr(params)),
                ("outputs", Value::Arr(outputs)),
                ("supports_seed", Value::Bool(def.supports_seed)),
                ("available", Value::Bool(c.available)),
                ("unavailable_reason", match c.reason {
                    Some(r) => s(r),
                    None => Value::Null,
                }),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(200, &obj(vec![("types", Value::Arr(types))]))))
}

// ---------------------------------------------------------------------------
// strict create parsing
// ---------------------------------------------------------------------------

/// Refuse any key outside the allowed set: this surface fails closed.
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

fn input_of(v: &Value) -> RouteResult<OperationInputBinding> {
    check_known_fields(
        v,
        &["slot", "asset", "revision", "role", "tier", "lod", "media"],
        "unknown operation input field",
    )?;
    let slot = body_str(v, "slot")?.to_string();
    let asset_id = AssetId::from_str(body_str(v, "asset")?)
        .map_err(|_| Fail::Http(400, "malformed asset id"))?;
    let revision = AssetRevisionId::from_str(body_str(v, "revision")?)
        .map_err(|_| Fail::Http(400, "malformed revision id"))?;
    let role =
        parse_role(body_str(v, "role")?).ok_or(Fail::Http(400, "malformed input role"))?;
    let tier = match v.get("tier") {
        None => None,
        Some(t) => Some(
            parse_tier(t.as_str().ok_or(Fail::Http(400, "malformed input tier"))?)
                .ok_or(Fail::Http(400, "malformed input tier"))?,
        ),
    };
    let lod = match v.get("lod") {
        None => None,
        Some(l) => {
            let n = l.as_u64().ok_or(Fail::Http(400, "malformed input lod"))?;
            if n > u8::MAX as u64 {
                return Err(Fail::Http(400, "malformed input lod"));
            }
            Some(n as u8)
        }
    };
    let expected_media = match v.get("media") {
        None => None,
        Some(m) => Some(
            parse_media(m.as_str().ok_or(Fail::Http(400, "malformed input media"))?)
                .ok_or(Fail::Http(400, "malformed input media"))?,
        ),
    };
    Ok(OperationInputBinding { slot, asset_id, revision, role, tier, lod, expected_media })
}

fn params_of(v: &Value) -> RouteResult<Vec<(String, ParamValue)>> {
    let Some(params) = v.get("params") else {
        return Ok(Vec::new());
    };
    let Value::Obj(pairs) = params else {
        return Err(Fail::Http(400, "params must be an object"));
    };
    let mut out = Vec::with_capacity(pairs.len());
    for (name, value) in pairs {
        let typed = match value {
            Value::Int(i) => ParamValue::Int(*i),
            Value::Str(t) => ParamValue::Text(t.clone()),
            Value::Bool(b) => ParamValue::Bool(*b),
            _ => return Err(Fail::Http(400, "unsupported parameter value")),
        };
        out.push((name.clone(), typed));
    }
    Ok(out)
}

fn publication_of(v: &Value) -> RouteResult<OperationPublication> {
    let Some(publication) = v.get("publication") else {
        return Ok(OperationPublication::Publish);
    };
    check_known_fields(
        publication,
        &["mode", "alias", "expect", "expect_head"],
        "unknown publication field",
    )?;
    match body_str(publication, "mode")? {
        "publish" => {
            if publication.get("alias").is_some()
                || publication.get("expect").is_some()
                || publication.get("expect_head").is_some()
            {
                return Err(Fail::Http(400, "publish takes no alias fields"));
            }
            Ok(OperationPublication::Publish)
        }
        "publish_and_alias" => {
            let alias = AssetAlias::from_str(body_str(publication, "alias")?)
                .map_err(|_| Fail::Http(400, "malformed alias"))?;
            let expect = match publication.get("expect").map(|e| e.as_str()) {
                None | Some(Some("any")) => {
                    if publication.get("expect_head").is_some() {
                        return Err(Fail::Http(400, "expect_head without expect head"));
                    }
                    AliasExpect::Unconditional
                }
                Some(Some("absent")) => {
                    if publication.get("expect_head").is_some() {
                        return Err(Fail::Http(400, "expect_head without expect head"));
                    }
                    AliasExpect::Absent
                }
                Some(Some("head")) => {
                    let head = AssetRevisionId::from_str(body_str(publication, "expect_head")?)
                        .map_err(|_| Fail::Http(400, "malformed expect_head"))?;
                    AliasExpect::Head(head)
                }
                _ => return Err(Fail::Http(400, "malformed publication expect")),
            };
            Ok(OperationPublication::PublishAndAlias { alias, expect })
        }
        _ => Err(Fail::Http(400, "malformed publication mode")),
    }
}

// ---------------------------------------------------------------------------
// job arming (the meta-then-enqueue pattern, shared by create and retry)
// ---------------------------------------------------------------------------

/// Build the executor job body: everything a worker needs to fetch its exact
/// inputs and understand its output contract. Pure projection of the pinned
/// snapshot — the worker never resolves anything itself.
fn job_body_of(snapshot: &OperationSnapshot, job: &ArmedJob) -> Value {
    let inputs: Vec<Value> = snapshot
        .inputs
        .iter()
        .map(|p| {
            obj(vec![
                ("slot", s(p.slot.clone())),
                ("asset", s(p.asset_id.to_string())),
                ("revision", s(p.revision.to_string())),
                ("role", s(role_str(p.role))),
                ("tier", s(tier_str(p.tier))),
                ("lod", Value::Int(p.lod as i64)),
                ("media", s(media_str(p.media))),
                ("blob", s(p.blob.to_string())),
                ("byte_len", Value::Int(p.byte_len.min(i64::MAX as u64) as i64)),
            ])
        })
        .collect();
    let params: Vec<(String, Value)> = snapshot
        .params
        .iter()
        .map(|(name, value)| {
            let v = match value {
                ParamValue::Int(i) => Value::Int(*i),
                ParamValue::Text(t) => s(t.clone()),
                ParamValue::Bool(b) => Value::Bool(*b),
            };
            (name.clone(), v)
        })
        .collect();
    let outputs: Vec<Value> = crate::operations::operation_def(&snapshot.kind)
        .map(|def| {
            def.outputs
                .iter()
                .map(|o| {
                    obj(vec![
                        ("name", s(o.name)),
                        ("kind", s(super::api::kind_str(o.kind))),
                        (
                            "required_roles",
                            Value::Arr(o.required_roles.iter().map(|r| s(role_str(*r))).collect()),
                        ),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    obj(vec![
        ("operation", s(operation_str(&snapshot.id))),
        ("operation_kind", s(snapshot.kind.clone())),
        ("round", Value::Int(job.round as i64)),
        ("inputs", Value::Arr(inputs)),
        ("params", Value::Obj(params)),
        ("outputs", Value::Arr(outputs)),
    ])
}

/// Enqueue an armed operation job through the SAME invariants the job route
/// enforces: metadata row first (the claim gate and job visibility read it),
/// core enqueue second, meta rolled back on enqueue refusal. An existing job
/// under the same id is the crash-repair replay joining its round.
fn enqueue_armed(
    ctx: &mut StateCtx,
    snapshot: &OperationSnapshot,
    job: &ArmedJob,
    p: &PrincipalId,
    now: u64,
) -> Result<(), ServerError> {
    let ns = snapshot.namespace.clone();
    let body = job_body_of(snapshot, job);
    let payload = envelope_build(&ns, p, &body);
    let distinct = ctx.meta_distinct_ns()?;
    if !distinct.iter().any(|n| n == &ns) && distinct.len() as u64 >= MAX_JOB_NAMESPACES {
        return Err(ServerError::OverBudget {
            what: "job namespaces",
            limit: MAX_JOB_NAMESPACES,
            found: distinct.len() as u64 + 1,
        });
    }
    if ctx.meta_get(&job.job_id)?.is_none() {
        ctx.meta_insert(&job.job_id, &ns, job.kind, p, now)?;
    }
    let enqueue = ctx.core.jobs().enqueue(
        &NewJob {
            job_id: job.job_id,
            parent: None,
            kind: job.kind,
            payload: &payload,
            priority: 0,
            max_attempts: OPERATION_JOB_ATTEMPTS,
            not_before_ms: 0,
            deps: &[],
        },
        now,
    );
    match enqueue {
        Ok(()) => Ok(()),
        // Crash-repair replay: the job already exists from a prior arming of
        // this same round — joining it is the point.
        Err(ServerError::Conflict { what: "job id" }) => Ok(()),
        Err(e) => {
            ctx.meta_delete(&job.job_id)?;
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

pub fn operation_create(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    check_known_fields(
        &body,
        &["api_version", "namespace", "kind", "idempotency_key", "inputs", "params", "publication"],
        "unknown operation field",
    )?;
    match body_u64(&body, "api_version") {
        Some(1) => {}
        _ => return Err(Fail::Http(400, "unsupported api_version")),
    }
    let ns = body_str(&body, "namespace")?.to_string();
    let kind = body_str(&body, "kind")?.to_string();
    let idempotency_key = body_str(&body, "idempotency_key")?.to_string();
    let inputs_v = body
        .get("inputs")
        .and_then(Value::as_arr)
        .ok_or(Fail::Http(400, "missing inputs"))?;
    let mut inputs = Vec::with_capacity(inputs_v.len());
    for entry in inputs_v {
        inputs.push(input_of(entry)?);
    }
    let params = params_of(&body)?;
    let publication = publication_of(&body)?;
    let operation_id = OperationId(rand16().map_err(Fail::Srv)?);
    let now = now_ms();

    let outcome = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::OperationRun, &ns)?;
        let outcome = ctx.core.operations().create(
            &OperationCreateRequest {
                operation_id,
                owner: p,
                namespace: &ns,
                kind: &kind,
                idempotency_key: &idempotency_key,
                inputs: &inputs,
                params: &params,
                publication,
            },
            now,
        )?;
        match &outcome {
            OperationCreateOutcome::Created { snapshot, job } => {
                enqueue_armed(ctx, snapshot, job, &p, now)?;
            }
            OperationCreateOutcome::Joined { snapshot, rearm: Some(job) } => {
                enqueue_armed(ctx, snapshot, job, &p, now)?;
            }
            OperationCreateOutcome::Joined { .. } => {}
        }
        Ok(outcome)
    })?;
    let (status, snapshot, joined) = match &outcome {
        OperationCreateOutcome::Created { snapshot, .. } => (201, snapshot, false),
        OperationCreateOutcome::Joined { snapshot, .. } => (200, snapshot, true),
    };
    let mut value = snapshot_value(snapshot, None);
    if let Value::Obj(pairs) = &mut value {
        pairs.push(("joined".to_string(), Value::Bool(joined)));
    }
    Ok(Outcome::Resp(Resp::json(status, &value)))
}

// ---------------------------------------------------------------------------
// status / events
// ---------------------------------------------------------------------------

fn snapshot_value(snapshot: &OperationSnapshot, progress: Option<(u64, String, u64)>) -> Value {
    let inputs: Vec<Value> = snapshot
        .inputs
        .iter()
        .map(|p| {
            obj(vec![
                ("slot", s(p.slot.clone())),
                ("asset", s(p.asset_id.to_string())),
                ("revision", s(p.revision.to_string())),
                ("role", s(role_str(p.role))),
                ("tier", s(tier_str(p.tier))),
                ("lod", Value::Int(p.lod as i64)),
                ("media", s(media_str(p.media))),
            ])
        })
        .collect();
    let mut pairs = vec![
        ("operation", s(operation_str(&snapshot.id))),
        ("namespace", s(snapshot.namespace.clone())),
        ("kind", s(snapshot.kind.clone())),
        ("state", s(snapshot.display_state)),
        ("round", Value::Int(snapshot.round as i64)),
        ("job", s(super::api::job_str(&snapshot.job_id))),
        ("owner", s(principal_str(&snapshot.owner))),
        ("idempotency_key", s(snapshot.idempotency_key.clone())),
        ("spec_digest", s(to_hex(&snapshot.spec_digest))),
        ("created_ms", Value::Int(snapshot.created_ms.min(i64::MAX as u64) as i64)),
        ("updated_ms", Value::Int(snapshot.updated_ms.min(i64::MAX as u64) as i64)),
        ("inputs", Value::Arr(inputs)),
    ];
    if let Some(e) = &snapshot.error {
        pairs.push(("error", s(e.clone())));
    }
    if let Some((asset, revision)) = &snapshot.result {
        pairs.push((
            "result",
            obj(vec![
                ("asset", s(asset.to_string())),
                ("revision", s(revision.to_string())),
            ]),
        ));
    }
    if let Some((permille, note, updated)) = progress {
        pairs.push((
            "progress",
            obj(vec![
                ("permille", Value::Int(permille.min(1000) as i64)),
                ("note", s(note)),
                ("updated_ms", Value::Int(updated.min(i64::MAX as u64) as i64)),
            ]),
        ));
    }
    obj(pairs)
}

pub fn operation_get(head: &Head, rc: &RouteCtx, id: OperationId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (snapshot, progress) = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let snapshot = ctx.core.operations().get(&p, &id, now)?;
        let progress = ctx
            .progress_get(&snapshot.job_id)?
            .map(|row| (row.permille, row.note, row.updated_ms));
        Ok((snapshot, progress))
    })?;
    Ok(Outcome::Resp(Resp::json(200, &snapshot_value(&snapshot, progress))))
}

pub fn operation_events(head: &Head, rc: &RouteCtx, id: OperationId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
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
            n.min(rc.cfg.event_max_wait_ms)
        }
    };
    let limit = super::api::parse_limit(head.query_get("limit"), 64, 256)? as u32;

    // Bounded long-poll: each slice is one state call; the FIRST call also
    // proves authentication and ownership (any refusal returns immediately).
    // Parking past the first empty answer requires a waiter-budget slot, so
    // event waiters can never occupy the whole control connection cap.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut slot: Option<WaiterSlot<'_>> = None;
    loop {
        let secret_now = secret.clone();
        let now = now_ms();
        let rows = call_state(&rc.state, move |ctx| {
            let p = ctx.core.auth().authenticate(secret_now.as_bytes(), now)?;
            ctx.core.operations().events(&p, &id, after, limit, now)
        })?;
        let park = rows.is_empty()
            && std::time::Instant::now() < deadline
            && match &slot {
                Some(_) => true,
                None => {
                    slot = WaiterSlot::acquire(&rc.op_event_waiters);
                    slot.is_some()
                }
            };
        if !park {
            let events: Vec<Value> = rows
                .iter()
                .map(|e| {
                    obj(vec![
                        ("seq", Value::Int(e.seq.min(i64::MAX as u64) as i64)),
                        ("kind", s(e.kind.clone())),
                        ("detail", s(e.detail.clone())),
                        ("created_ms", Value::Int(e.created_ms.min(i64::MAX as u64) as i64)),
                    ])
                })
                .collect();
            let cursor = rows.last().map(|e| e.seq).unwrap_or(after);
            return Ok(Outcome::Resp(Resp::json(
                200,
                &obj(vec![
                    ("events", Value::Arr(events)),
                    ("cursor", Value::Int(cursor.min(i64::MAX as u64) as i64)),
                ]),
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(
            EVENT_WAIT_SLICE_MS.min(wait_ms.max(1)),
        ));
    }
}

// ---------------------------------------------------------------------------
// cancel / retry
// ---------------------------------------------------------------------------

pub fn operation_cancel(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    id: OperationId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    // Cancel takes no body fields; still drain/refuse a body per read_json.
    let _body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let now = now_ms();
    let cancelled = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let snapshot = ctx.core.operations().get(&p, &id, now)?;
        require_cap(ctx, &p, Capability::OperationRun, &snapshot.namespace)?;
        ctx.core.operations().cancel(&p, &id, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("cancelled", Value::Bool(cancelled))]),
    )))
}

pub fn operation_retry(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    id: OperationId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let _body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let now = now_ms();
    let snapshot = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let snapshot = ctx.core.operations().get(&p, &id, now)?;
        require_cap(ctx, &p, Capability::OperationRun, &snapshot.namespace)?;
        let (snapshot, job) = ctx.core.operations().retry(&p, &id, now)?;
        enqueue_armed(ctx, &snapshot, &job, &p, now)?;
        Ok(snapshot)
    })?;
    Ok(Outcome::Resp(Resp::json(200, &snapshot_value(&snapshot, None))))
}

// ---------------------------------------------------------------------------
// worker finalize
// ---------------------------------------------------------------------------

fn output_file_of(v: &Value) -> RouteResult<AssetFile> {
    check_known_fields(
        v,
        &["role", "tier", "lod", "media", "blob", "byte_len", "dims"],
        "unknown output file field",
    )?;
    let role = parse_role(body_str(v, "role")?).ok_or(Fail::Http(400, "malformed output role"))?;
    let tier = match v.get("tier") {
        None => DeviceTier::Any,
        Some(t) => parse_tier(t.as_str().ok_or(Fail::Http(400, "malformed output tier"))?)
            .ok_or(Fail::Http(400, "malformed output tier"))?,
    };
    let lod = body_u64(v, "lod").unwrap_or(0);
    if lod > u8::MAX as u64 {
        return Err(Fail::Http(400, "malformed output lod"));
    }
    let media =
        parse_media(body_str(v, "media")?).ok_or(Fail::Http(400, "malformed output media"))?;
    let blob = body_str(v, "blob")?
        .parse()
        .map_err(|_| Fail::Http(400, "malformed blob id"))?;
    let byte_len = body_u64(v, "byte_len").ok_or(Fail::Http(400, "missing output byte_len"))?;
    let dims = match v.get("dims") {
        None => None,
        Some(d) => Some(ImageDims {
            width: body_u64(d, "width").ok_or(Fail::Http(400, "malformed output dims"))? as u32,
            height: body_u64(d, "height").ok_or(Fail::Http(400, "malformed output dims"))? as u32,
        }),
    };
    Ok(AssetFile { role, tier, lod: lod as u8, media, blob, byte_len, dims })
}

fn metrics_of(v: &Value) -> RouteResult<Metrics> {
    let m = v.get("metrics").ok_or(Fail::Http(400, "missing metrics"))?;
    let field = |key: &str| body_u64(m, key).unwrap_or(0);
    Ok(Metrics {
        total_bytes: body_u64(m, "total_bytes").ok_or(Fail::Http(400, "missing total_bytes"))?,
        triangles: field("triangles") as u32,
        vertices: field("vertices") as u32,
        joints: field("joints") as u16,
        clips: field("clips") as u16,
        max_texture_dim: field("max_texture_dim") as u32,
        media_millis: field("media_millis") as u32,
    })
}

fn vec3_of(v: &Value, what: &'static str) -> RouteResult<Vec3> {
    let arr = v.as_arr().ok_or(Fail::Http(400, what))?;
    if arr.len() != 3 {
        return Err(Fail::Http(400, what));
    }
    let f = |i: usize| -> RouteResult<f32> {
        match &arr[i] {
            Value::Int(n) => Ok(*n as f32),
            Value::F64(n) => Ok(*n as f32),
            _ => Err(Fail::Http(400, what)),
        }
    };
    Ok(Vec3::new(f(0)?, f(1)?, f(2)?))
}

fn output_facts_of(name: &str, v: &Value) -> RouteResult<OperationOutputFacts> {
    check_known_fields(
        v,
        &["files", "thumbnail", "metrics", "bounds"],
        "unknown output field",
    )?;
    let files_v = v
        .get("files")
        .and_then(Value::as_arr)
        .ok_or(Fail::Http(400, "missing output files"))?;
    let mut files = Vec::with_capacity(files_v.len());
    for entry in files_v {
        files.push(output_file_of(entry)?);
    }
    let thumbnail = match v.get("thumbnail") {
        None => None,
        Some(t) => Some(ThumbnailMeta {
            blob: body_str(t, "blob")?
                .parse()
                .map_err(|_| Fail::Http(400, "malformed thumbnail blob"))?,
            media: match body_str(t, "media")? {
                "png" => ThumbnailMedia::Png,
                "jpeg" => ThumbnailMedia::Jpeg,
                _ => return Err(Fail::Http(400, "malformed thumbnail media")),
            },
            width: body_u64(t, "width").ok_or(Fail::Http(400, "malformed thumbnail"))? as u32,
            height: body_u64(t, "height").ok_or(Fail::Http(400, "malformed thumbnail"))? as u32,
            byte_len: body_u64(t, "byte_len").ok_or(Fail::Http(400, "malformed thumbnail"))?,
        }),
    };
    let metrics = metrics_of(v)?;
    let bounds = match v.get("bounds") {
        None => None,
        Some(b) => Some(Bounds {
            min: vec3_of(b.get("min").ok_or(Fail::Http(400, "malformed bounds"))?, "malformed bounds")?,
            max: vec3_of(b.get("max").ok_or(Fail::Http(400, "malformed bounds"))?, "malformed bounds")?,
        }),
    };
    Ok(OperationOutputFacts { name: name.to_string(), files, thumbnail, metrics, bounds })
}

fn facts_of(body: &Value) -> RouteResult<OperationResultFacts> {
    let outputs_v = body.get("outputs").ok_or(Fail::Http(400, "missing outputs"))?;
    let Value::Obj(pairs) = outputs_v else {
        return Err(Fail::Http(400, "outputs must be an object"));
    };
    let mut outputs = Vec::with_capacity(pairs.len());
    for (name, v) in pairs {
        outputs.push(output_facts_of(name, v)?);
    }
    let model = body.get("model").ok_or(Fail::Http(400, "missing model facts"))?;
    check_known_fields(
        model,
        &["generator", "model", "version", "seed"],
        "unknown model field",
    )?;
    Ok(OperationResultFacts {
        outputs,
        generator: body_str(model, "generator")?.to_string(),
        model: body_str(model, "model")?.to_string(),
        version: body_str(model, "version")?.to_string(),
        seed: body_u64(model, "seed").ok_or(Fail::Http(400, "missing model seed"))?,
    })
}

pub fn operation_finalize(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    id: OperationId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let job = parse_job(body_str(&body, "job")?).ok_or(Fail::Http(400, "malformed job id"))?;
    let suffix = worker_suffix(&body)?;
    let facts = facts_of(&body)?;
    let now = now_ms();
    let hub = rc.events.clone();
    let (asset, revision) = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        // The OPERATION's namespace is the authority the capability check
        // and the mirrors bind to; the transport meta row must agree (it
        // always does today — enqueue_armed writes it from the snapshot —
        // and this assert keeps that an enforced invariant, not a habit).
        let snapshot = ctx.core.operations().executor_snapshot(&id)?;
        if meta.ns != snapshot.namespace {
            return Err(ServerError::Conflict { what: "operation job namespace" });
        }
        require_cap(ctx, &p, Capability::JobWorker, &snapshot.namespace)?;
        let worker = worker_name(&p, &suffix);
        let (asset, revision) =
            ctx.core.operations().finalize(&id, &job, &worker, &facts, now)?;

        // Post-commit mirrors + events, still on the state thread so journal
        // order equals commit order (the same convention as every other
        // publishing route).
        ctx.asset_index_insert(asset.as_bytes(), &meta.ns, now)?;
        ctx.asset_rev_insert(asset.as_bytes(), revision.as_bytes(), now)?;
        ctx.asset_rev_mark(asset.as_bytes(), revision.as_bytes(), true, now)?;
        let attempt = ctx
            .core
            .jobs()
            .attempts(&job)?
            .last()
            .map(|a| a.attempt as u64)
            .unwrap_or(0);
        let doc = obj(vec![
            ("asset_id", s(asset.to_string())),
            ("revision", s(revision.to_string())),
        ])
        .to_json()
        .into_bytes();
        ctx.result_set(&job, "succeeded", attempt, &doc, now)?;

        // Searchable annotation so the output is discoverable: title from
        // the operation's own pinned params (immutable, so the pre-finalize
        // snapshot above serves), model facts from the run.
        let title = snapshot
            .params
            .iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("title", ParamValue::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_else(|| format!("{} output", snapshot.kind));
        let def = crate::operations::operation_def(&snapshot.kind);
        let out_kind = def.and_then(|d| d.outputs.first()).map(|o| o.kind);
        let annotation = crate::AssetAnnotation {
            title,
            description: String::new(),
            kind: out_kind,
            categories: vec!["generated".to_string()],
            tags: Vec::new(),
            creator: String::new(),
            owner: Some(snapshot.owner),
            generator: facts.generator.clone(),
            backend: String::new(),
            model: facts.model.clone(),
            prompt: String::new(),
            provenance: String::new(),
            visibility: crate::Visibility::Public,
        };
        ctx.core.search().set_annotation(&asset, &annotation, now)?;

        hub.publish(
            EventBody::asset(events::KIND_ASSET_PUBLISHED, &meta.ns, asset.to_string(), now)
                .with_revision(revision.to_string())
                .with_content_kind(out_kind.map(super::api::kind_str)),
        );
        if let OperationPublication::PublishAndAlias { alias, .. } = &snapshot.publication {
            hub.publish(
                EventBody::asset(events::KIND_ALIAS_SET, &meta.ns, asset.to_string(), now)
                    .with_revision(revision.to_string())
                    .with_alias(alias.to_string())
                    .with_content_kind(out_kind.map(super::api::kind_str)),
            );
        }
        Ok((asset, revision))
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset", s(asset.to_string())),
            ("revision", s(revision.to_string())),
        ]),
    )))
}
