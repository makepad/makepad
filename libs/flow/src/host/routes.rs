use super::backend::{Backend, FileError};
use super::config::SharedConfig;
use super::events::{EventCursor, EventHub};
use super::util::log;
use crate::document::closure::Closure;
use crate::runtime::store::SourceResult;
use crate::runtime::{Conflict, EngineCommand, EngineError, EngineQuery, ReplyPayload};
use crate::{
    CreateFromTemplateRequest, CreateInstanceRequest, CreateInstanceResponse,
    CreateBatchRequest, CreateRunRequest, CreateRunResponse, EvalErrorResponse, EventsResponse, FlowMutationResponse,
    HealthResponse, InstanceId, MessageResponse, NodesResponse,
    PortType, PutGraphRequest, PutSourceRequest, PutValueResponse, RevertRequest, RunId,
    TemplateResponse, Value,
};
use crate::engine::results::{ResultQuery, ResultRef, ResultStore};
use makepad_bounded_http::{
    etag_matches, if_range_matches, parse_range, BodyError, Conn, Head, Method, RangeSpec, Resp,
};
use makepad_micro_serde::{DeJson, SerJson};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const BODY_DEADLINE_MS: u64 = 30_000;
const MAX_CONTROL_BODY_BYTES: u64 = 1024 * 1024;
const EVENT_MAX_WAIT_MS: u64 = 30_000;
const EVENT_MAX_BATCH: usize = 256;
/// `PUT /v1/values` body cap: media only, generously above any test asset,
/// well under the control plane's 1 MiB (the store's own blob cap shape).
const MAX_VALUE_BYTES: u64 = 64 * 1024 * 1024;
/// Values are scratch (§5.5): no long-lived caching contract.
const VALUE_CACHE_CONTROL: &str = "private, max-age=60";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plane {
    Control,
    Data,
}

#[derive(Clone)]
pub(crate) struct RouteCtx {
    pub backend: Arc<Backend>,
    pub config: SharedConfig,
    pub server_id: [u8; 16],
    pub token: String,
    pub events: Arc<EventHub>,
    pub results: Arc<dyn ResultStore>,
}

pub(crate) enum Outcome {
    Resp(Resp),
    Hangup,
}

pub(crate) fn dispatch(
    conn: &mut Conn,
    head: &mut Head,
    ctx: &RouteCtx,
    plane: Plane,
) -> Outcome {
    if plane == Plane::Control
        && head.method == Method::Get
        && head.segs.as_slice() == ["v1", "health"]
    {
        return live(
            ctx,
            json(
                200,
                &HealthResponse {
                    service: "makepad-flow".to_string(),
                    server_id: super::util::to_hex(&ctx.server_id),
                    protocol_version: 1,
                    revision_epoch: ctx.backend.epoch,
                },
            ),
        );
    }

    if !bearer_matches(head.authorization.as_deref(), &ctx.token) {
        return Outcome::Resp(message(401, "unauthorized"));
    }

    if plane == Plane::Data {
        return data_dispatch(conn, head, ctx);
    }

    let segments = head.segs.clone();
    match segments.as_slice() {
        [v1, assets] if v1 == "v1" && assets == "assets" => {
            let (q, namespace, limit, cursor) = if head.method == Method::Post {
                let request: crate::AssetsRequest = match read_json(conn, head) {
                    Ok(request) => request,
                    Err(outcome) => return outcome,
                };
                if request.limit == 0 || request.limit > 100 || request.query.len() > 4096 {
                    return Outcome::Resp(message(400, "invalid asset query"));
                }
                (request.query, request.namespace, request.limit, request.cursor)
            } else if head.method == Method::Get {
                let q = head.query_get("q").unwrap_or("").replace('+', " ");
                let namespace = match head.query_get("ns") {
                    Some("*") => None,
                    Some(value) if !value.is_empty() => Some(value.to_string()),
                    _ => Some("flows".to_string()),
                };
                let limit = match decimal_query(head.query_get("limit"), 50, 100) {
                    Some(value) if value > 0 => value as u32,
                    _ => return Outcome::Resp(message(400, "invalid asset limit")),
                };
                (q, namespace, limit, head.query_get("cursor").map(str::to_string))
            } else {
                return Outcome::Resp(message(405, "method not allowed"));
            };
            if cursor.as_ref().is_some_and(|cursor| cursor.len() > 8192) {
                return Outcome::Resp(message(400, "invalid asset cursor"));
            }
            match ctx.results.list(ResultQuery { text: q, namespace, kind: None, limit, cursor }) {
                Ok(page) => Outcome::Resp(json(200, &crate::AssetsResponse { assets: page.items, cursor: page.cursor })),
                Err(error) => Outcome::Resp(message(store_error_status(&error), &error)),
            }
        }
        [v1, assets, thumb, id]
            if v1 == "v1" && assets == "assets" && thumb == "thumbnail" =>
        {
            asset_content(head, ctx, id, true)
        }
        [v1, assets, thumb, tail @ ..]
            if v1 == "v1" && assets == "assets" && thumb == "thumb" =>
        {
            if head.method != Method::Get {
                return Outcome::Resp(message(405, "method not allowed"));
            }
            let alias = tail.join("/");
            if alias.is_empty() || alias.len() > 256 || alias.chars().any(char::is_control) {
                return Outcome::Resp(message(400, "invalid asset alias"));
            }
            match ctx.results.thumbnail(&ResultRef::alias(alias)) {
                Ok(Some(thumbnail)) => Outcome::Resp(
                    Resp::bytes(200, &thumbnail.content_type, thumbnail.bytes)
                        .with_header("Cache-Control", "private, max-age=300".to_string()),
                ),
                Ok(None) => Outcome::Resp(message(404, "asset thumbnail not found")),
                Err(error) => Outcome::Resp(message(store_error_status(&error), &error)),
            }
        }
        [v1, health] if v1 == "v1" && health == "health" => {
            live(ctx, message(405, "method not allowed"))
        }
        [v1, nodes] if v1 == "v1" && nodes == "nodes" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            if !ctx.backend.is_alive() {
                return unavailable();
            }
            let types = ctx.backend.catalog_with_models();
            Outcome::Resp(json(
                200,
                &NodesResponse {
                    types,
                    registry: Some(ctx.backend.registry_data()),
                    brief: crate::AUTHORING_BRIEF.to_string(),
                },
            ))
        }
        [v1, models] if v1 == "v1" && models == "models" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            if !ctx.backend.is_alive() {
                return unavailable();
            }
            let response = ctx.backend.models_response(head.query_get("domain"));
            Outcome::Resp(json(200, &response))
        }
        [v1, templates] if v1 == "v1" && templates == "templates" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            let mut summaries: Vec<_> = crate::templates::TEMPLATES
                .iter()
                .chain(ctx.config.extra_templates.iter())
                .map(crate::templates::template_summary)
                .collect();
            summaries.sort_by(|left, right| {
                crate::templates::group_rank(&left.group)
                    .cmp(&crate::templates::group_rank(&right.group))
                    .then_with(|| left.name.cmp(&right.name))
            });
            Outcome::Resp(json(200, &summaries))
        }
        [v1, templates, name] if v1 == "v1" && templates == "templates" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            let Some(template) = crate::templates::template_in(&ctx.config.extra_templates, name) else {
                return Outcome::Resp(message(404, "template not found"));
            };
            let summary = crate::templates::template_summary(template);
            Outcome::Resp(json(
                200,
                &TemplateResponse {
                    name: summary.name,
                    label: summary.label,
                    brief: summary.brief,
                    source: template.source.to_string(),
                },
            ))
        }
        [v1, flows] if v1 == "v1" && flows == "flows" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            match ctx.backend.query(EngineQuery::Definitions) {
                Ok(ReplyPayload::Definitions(rows)) => Outcome::Resp(json(200, &rows)),
                _ => unavailable(),
            }
        }
        [v1, events] if v1 == "v1" && events == "events" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            events_route(head, ctx)
        }
        [v1, flows, name] if v1 == "v1" && flows == "flows" => {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            match head.method {
                Method::Get => get_flow(ctx, name),
                Method::Post => create_from_template(conn, head, ctx, name),
                Method::Put => put_source(conn, head, ctx, name),
                Method::Delete => delete_flow(ctx, name),
                _ => live(ctx, message(405, "method not allowed")),
            }
        }
        [v1, flows, name, graph_segment]
            if v1 == "v1" && flows == "flows" && graph_segment == "graph" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Put {
                return live(ctx, message(405, "method not allowed"));
            }
            put_graph(conn, head, ctx, name)
        }
        [v1, flows, name, revert]
            if v1 == "v1" && flows == "flows" && revert == "revert" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            revert_flow(conn, head, ctx, name)
        }
        [v1, flows, name, instances_segment]
            if v1 == "v1" && flows == "flows" && instances_segment == "instances" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            create_instance(conn, head, ctx, name)
        }
        [v1, flows, name, parallelism]
            if v1 == "v1" && flows == "flows" && parallelism == "parallelism" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            parallelism_route(ctx, name)
        }
        [v1, flows, name, batches]
            if v1 == "v1" && flows == "flows" && batches == "batches" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            create_batch(conn, head, ctx, name)
        }
        [v1, batches, id, cancel]
            if v1 == "v1" && batches == "batches" && cancel == "cancel" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid batch id"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            cancel_batch_route(ctx, id)
        }
        [v1, batches, id] if v1 == "v1" && batches == "batches" => {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid batch id"));
            }
            if head.method != Method::Delete {
                return live(ctx, message(405, "method not allowed"));
            }
            clear_batch_route(ctx, id)
        }
        [v1, instances] if v1 == "v1" && instances == "instances" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            list_instances(head, ctx)
        }
        [v1, instances, id] if v1 == "v1" && instances == "instances" => {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid instance id"));
            }
            match head.method {
                Method::Get => get_instance(ctx, id),
                Method::Delete => delete_instance(ctx, id),
                _ => live(ctx, message(405, "method not allowed")),
            }
        }
        [v1, instances, id, inputs]
            if v1 == "v1" && instances == "instances" && inputs == "inputs" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid instance id"));
            }
            if head.method != Method::Put {
                return live(ctx, message(405, "method not allowed"));
            }
            put_instance_inputs(conn, head, ctx, id)
        }
        [v1, instances, id, clear]
            if v1 == "v1" && instances == "instances" && clear == "clear" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid instance id"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            clear_instance(ctx, id)
        }
        [v1, instances, id, runs_segment]
            if v1 == "v1" && instances == "instances" && runs_segment == "runs" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid instance id"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            create_run(conn, head, ctx, id)
        }
        [v1, runs] if v1 == "v1" && runs == "runs" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            list_runs(head, ctx)
        }
        [v1, runs, id] if v1 == "v1" && runs == "runs" => {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid run id"));
            }
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            get_run(ctx, id)
        }
        [v1, runs, id, cancel]
            if v1 == "v1" && runs == "runs" && cancel == "cancel" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid run id"));
            }
            if head.method != Method::Post {
                return live(ctx, message(405, "method not allowed"));
            }
            cancel_run_route(ctx, id)
        }
        _ => live(ctx, message(404, "not found")),
    }
}

/// Data plane: value bytes only (§5.5, §6). Instance/run/flow control-plane
/// routes never touch this plane.
fn data_dispatch(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx) -> Outcome {
    let segments = head.segs.clone();
    match segments.as_slice() {
        [v1, assets, thumbnail, id]
            if v1 == "v1" && assets == "assets" && thumbnail == "thumbnail" =>
        {
            asset_content(head, ctx, id, true)
        }
        [v1, assets, id, content]
            if v1 == "v1" && assets == "assets" && content == "content" =>
        {
            asset_content(head, ctx, id, false)
        }
        [v1, assets, id, preview]
            if v1 == "v1" && assets == "assets" && preview == "preview" =>
        {
            asset_preview(head, ctx, id)
        }
        [v1, values, digest] if v1 == "v1" && values == "values" => {
            if head.method != Method::Get {
                return live(ctx, message(405, "method not allowed"));
            }
            get_value(head, ctx, digest)
        }
        [v1, values] if v1 == "v1" && values == "values" => {
            if head.method != Method::Put {
                return live(ctx, message(405, "method not allowed"));
            }
            put_value(conn, head, ctx)
        }
        _ => live(ctx, message(404, "not found")),
    }
}

/// A result id as a route segment: short, no separators.
fn result_ref(id: &str) -> Option<ResultRef> {
    let valid = !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    valid.then(|| ResultRef::id(id))
}

/// The status a store's error text maps to: `invalid` 400, `not found` 404,
/// `unavailable` 503, anything else 502.
fn store_error_status(error: &str) -> u16 {
    if error.contains("invalid") {
        400
    } else if error.contains("not found") {
        404
    } else if error.contains("unavailable") {
        503
    } else {
        502
    }
}

fn asset_content(head: &Head, ctx: &RouteCtx, id: &str, thumbnail: bool) -> Outcome {
    if head.method != Method::Get { return Outcome::Resp(message(405, "method not allowed")); }
    let Some(result) = result_ref(id) else { return Outcome::Resp(message(400, "invalid asset id")) };
    let read = if thumbnail {
        ctx.results.thumbnail(&result).and_then(|thumb| thumb.ok_or_else(|| "asset thumbnail not found".to_string()))
    } else {
        ctx.results.read(&result)
    };
    match read {
        Ok(value) => Outcome::Resp(Resp::bytes(200, &value.content_type, value.bytes)
            .with_header("Cache-Control", "private, max-age=0".to_string())),
        Err(error) => Outcome::Resp(message(store_error_status(&error), &error)),
    }
}

fn asset_preview(head: &Head, ctx: &RouteCtx, id: &str) -> Outcome {
    if head.method != Method::Get { return Outcome::Resp(message(405, "method not allowed")); }
    let Some(result) = result_ref(id) else { return Outcome::Resp(message(400, "invalid asset id")) };
    match ctx.results.preview(&result) {
        Ok(value) => Outcome::Resp(Resp::bytes(200, &value.content_type, value.bytes)
            .with_header("Cache-Control", "private, max-age=60".to_string())),
        Err(error) => Outcome::Resp(message(store_error_status(&error), &error)),
    }
}

fn get_flow(ctx: &RouteCtx, name: &str) -> Outcome {
    match ctx.backend.definition(name) {
        Ok(Some(detail)) => Outcome::Resp(json(200, &*detail)),
        Ok(None) => Outcome::Resp(message(404, "flow not found")),
        Err(_) => unavailable(),
    }
}

fn put_source(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: PutSourceRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    // A closure is resolved (dependency order, budgets, unknown aliases,
    // cycles) before anything reaches the engine; a refused closure
    // changes nothing.
    let closure = match request.modules {
        Some(modules) => match Closure::ordered(modules) {
            Ok(closure) => Some(Arc::new(closure)),
            Err(error) => return Outcome::Resp(json(400, &error)),
        },
        None => None,
    };
    match ctx.backend.put_source_with(name.to_string(), request.source, closure, request.ephemeral.unwrap_or(false)) {
        Ok(result) => Outcome::Resp(mutation_result(result)),
        Err(error) => file_error(ctx, "flow write failed", error),
    }
}

fn create_from_template(
    conn: &mut Conn,
    head: &mut Head,
    ctx: &RouteCtx,
    name: &str,
) -> Outcome {
    let request: CreateFromTemplateRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let Some(template) = crate::templates::template_in(&ctx.config.extra_templates, &request.template) else {
        return Outcome::Resp(message(404, "template not found"));
    };
    match ctx.backend.create_source(name.to_string(), template.source.to_string()) {
        Ok(Some(result)) => Outcome::Resp(create_mutation_result(result)),
        Ok(None) => Outcome::Resp(message(409, "flow already exists")),
        Err(error) => file_error(ctx, "flow create failed", error),
    }
}

fn put_graph(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: PutGraphRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    // Canonical roots use the graph writer. Custom roots admit only a
    // checked source patch to layout fields, under the same files lock.
    // Their surrounding code is preserved.
    match ctx.backend.put_graph(name.to_string(), &request.graph) {
        Ok(result) => Outcome::Resp(mutation_result(result)),
        Err(FileError::SourceOwned) => Outcome::Resp(message(
            409,
            "this flow contains authored code; only node layout can be edited through the graph, change its behavior in the source",
        )),
        Err(FileError::Invalid(why)) => Outcome::Resp(message(400, &format!("the graph is not valid: {why}"))),
        Err(error) => file_error(ctx, "flow graph write failed", error),
    }
}


fn revert_flow(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: RevertRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    match ctx.backend.revert(name, request.revision) {
        Ok(Some(result)) => Outcome::Resp(mutation_result(result)),
        Ok(None) => Outcome::Resp(message(404, "revision not found")),
        Err(error) => file_error(ctx, "flow revert failed", error),
    }
}

fn delete_flow(ctx: &RouteCtx, name: &str) -> Outcome {
    match ctx.backend.remove(name) {
        Ok(true) => Outcome::Resp(Resp::empty(204)),
        Ok(false) => Outcome::Resp(message(404, "flow not found")),
        Err(error) => file_error(ctx, "flow delete failed", error),
    }
}

/// A file operation failed: I/O is logged and answered 500, as before; an
/// engine that cannot answer is 503.
fn file_error(ctx: &RouteCtx, what: &str, error: FileError) -> Outcome {
    match error {
        FileError::Io(error) => {
            log(&ctx.config, &format!("{what}: {error}"));
            Outcome::Resp(message(500, "internal server error"))
        }
        FileError::Engine(error) => engine_error(error),
        FileError::SourceOwned => Outcome::Resp(message(409, "the flow's source is not in the graph writer's form")),
        FileError::Invalid(why) => Outcome::Resp(message(400, &format!("the graph is not valid: {why}"))),
    }
}

fn mutation_result(result: SourceResult) -> Resp {
    match result.error {
        Some(error) => json(422, &EvalErrorResponse { error }),
        None => match result.graph {
            Some(graph) => json(
                200,
                &FlowMutationResponse { revision: result.revision, graph },
            ),
            None => message(500, "missing evaluated graph"),
        },
    }
}

fn create_mutation_result(result: SourceResult) -> Resp {
    match result.error {
        Some(error) => json(422, &EvalErrorResponse { error }),
        None => match result.graph {
            Some(graph) => json(
                201,
                &FlowMutationResponse { revision: result.revision, graph },
            ),
            None => message(500, "missing evaluated graph"),
        },
    }
}

fn events_route(head: &Head, ctx: &RouteCtx) -> Outcome {
    if !ctx.backend.is_alive() {
        return unavailable();
    }
    let wait_ms = match decimal_query(head.query_get("wait"), 0, EVENT_MAX_WAIT_MS) {
        Some(value) => value,
        None => return Outcome::Resp(message(400, "malformed wait")),
    };
    let limit = match decimal_query(head.query_get("limit"), EVENT_MAX_BATCH as u64, EVENT_MAX_BATCH as u64) {
        Some(0) | None => return Outcome::Resp(message(400, "malformed limit")),
        Some(value) => value as usize,
    };
    let topic = head.query_get("topic").map(str::to_string);
    let Some(cursor_text) = head.query_get("cursor") else {
        let cursor = ctx.events.tail_cursor();
        return Outcome::Resp(json(
            200,
            &EventsResponse { events: Vec::new(), cursor: cursor.render(), gap: false },
        ));
    };
    let Some(mut cursor) = EventCursor::parse(cursor_text) else {
        return Outcome::Resp(message(400, "malformed cursor"));
    };
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let poll = ctx.events.poll_after(cursor, topic.as_deref(), limit);
        if poll.gap || !poll.events.is_empty() {
            return Outcome::Resp(json(
                200,
                &EventsResponse {
                    events: poll.events.iter().map(|event| event.wire_value()).collect(),
                    cursor: poll.cursor.render(),
                    gap: poll.gap,
                },
            ));
        }
        cursor = poll.cursor;
        if wait_ms == 0 || Instant::now() >= deadline || !ctx.events.wait_beyond(cursor.seq, deadline) {
            return Outcome::Resp(json(
                200,
                &EventsResponse { events: Vec::new(), cursor: cursor.render(), gap: false },
            ));
        }
    }
}

fn decimal_query(text: Option<&str>, default: u64, max: u64) -> Option<u64> {
    match text {
        None => Some(default),
        Some(text)
            if !text.is_empty()
                && text.len() <= 20
                && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(text.parse::<u64>().ok()?.min(max))
        }
        Some(_) => None,
    }
}

fn read_json<T: DeJson>(conn: &mut Conn, head: &mut Head) -> Result<T, Outcome> {
    let bytes = match conn.read_body_full(head, MAX_CONTROL_BODY_BYTES, BODY_DEADLINE_MS) {
        Ok(bytes) => bytes,
        Err(BodyError::Timeout) => {
            return Err(Outcome::Resp(message(408, "body timeout").closing()))
        }
        Err(BodyError::Malformed) => {
            return Err(Outcome::Resp(message(400, "malformed body").closing()))
        }
        Err(BodyError::TooLarge) => {
            conn.drain_remaining(head, Instant::now() + Duration::from_millis(BODY_DEADLINE_MS));
            return Err(Outcome::Resp(message(413, "body too large").closing()));
        }
        Err(BodyError::Io) => return Err(Outcome::Hangup),
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Outcome::Resp(message(400, "body is not utf-8")))?;
    T::deserialize_json(text).map_err(|_| Outcome::Resp(message(400, "malformed json")))
}

fn bearer_matches(header: Option<&str>, token: &str) -> bool {
    let Some(secret) = header.and_then(|header| header.strip_prefix("Bearer ")) else {
        return false;
    };
    if secret.len() != token.len() {
        return false;
    }
    secret
        .as_bytes()
        .iter()
        .zip(token.as_bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// A route that touches no state still answers 503 once the engine is gone.
fn live(ctx: &RouteCtx, response: Resp) -> Outcome {
    if ctx.backend.is_alive() {
        Outcome::Resp(response)
    } else {
        unavailable()
    }
}

fn unavailable() -> Outcome {
    Outcome::Resp(message(503, "state unavailable"))
}

/// An engine error no route gives its own words to.
fn engine_error(error: EngineError) -> Outcome {
    match error {
        EngineError::Stopped | EngineError::Stopping | EngineError::Superseded => unavailable(),
        EngineError::Busy(what) => Outcome::Resp(message(503, &what)),
        EngineError::NotFound(_) => Outcome::Resp(message(404, "not found")),
        EngineError::Invalid(what) => Outcome::Resp(message(422, &what)),
        EngineError::Conflict(what) => Outcome::Resp(message(409, &what.to_string())),
    }
}

fn json<T: SerJson>(status: u16, body: &T) -> Resp {
    Resp::bytes(status, "application/json", body.serialize_json().into_bytes())
}

fn message(status: u16, error: &str) -> Resp {
    json(status, &MessageResponse { error: error.to_string() })
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_')
}

/// Instance and run ids are `Instance::new`/`Instance::request_run`'s own
/// `<prefix>_<16 lowercase hex>` shape; checked loosely (safe charset,
/// bounded length) rather than coupled to that exact format.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

// ---------------------------------------------------------------------------
// instances (§4.1, §6)
// ---------------------------------------------------------------------------

fn create_instance(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: CreateInstanceRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let inputs = request.inputs.unwrap_or_default();
    match ctx.backend.create_instance(name, request.label, request.pin.unwrap_or(false), inputs) {
        Ok(ReplyPayload::Instance(id)) => Outcome::Resp(json(201, &CreateInstanceResponse { instance: id.0 })),
        Err(EngineError::NotFound(_)) => Outcome::Resp(message(404, "flow not found")),
        Err(EngineError::Conflict(Conflict::FlowInvalid)) => Outcome::Resp(message(409, "flow has no valid graph")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

fn parallelism_route(ctx: &RouteCtx, name: &str) -> Outcome {
    match ctx.backend.parallelism(name) {
        Ok(Some(Some(response))) => Outcome::Resp(json(200, &response)),
        Ok(Some(None)) => Outcome::Resp(message(409, "flow has no valid graph")),
        Ok(None) => Outcome::Resp(message(404, "flow not found")),
        Err(error) => engine_error(error),
    }
}

fn create_batch(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: CreateBatchRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    match ctx.backend.create_batch(name, request) {
        Ok(ReplyPayload::Batch(response)) => Outcome::Resp(json(202, &response)),
        Err(EngineError::NotFound(_)) => Outcome::Resp(message(404, "flow not found")),
        Err(EngineError::Conflict(Conflict::FlowInvalid)) => Outcome::Resp(message(409, "flow has no valid graph")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

fn cancel_batch_route(ctx: &RouteCtx, id: &str) -> Outcome {
    batch_mutation(ctx.backend.request(EngineCommand::CancelBatch { batch: id.to_string() }))
}

fn clear_batch_route(ctx: &RouteCtx, id: &str) -> Outcome {
    batch_mutation(ctx.backend.request(EngineCommand::ClearBatch { batch: id.to_string() }))
}

fn batch_mutation(result: Result<ReplyPayload, EngineError>) -> Outcome {
    match result {
        Ok(ReplyPayload::BatchRuns(Some(runs))) => Outcome::Resp(json(200, &crate::BatchMutationResponse { runs })),
        Ok(ReplyPayload::BatchRuns(None)) => Outcome::Resp(message(404, "batch not found")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

fn list_instances(head: &Head, ctx: &RouteCtx) -> Outcome {
    let flow = head.query_get("flow").map(str::to_string);
    let waiting_only = head.query_get("waiting") == Some("1");
    match ctx.backend.query(EngineQuery::Instances { flow, waiting_only }) {
        Ok(ReplyPayload::Instances(rows)) => Outcome::Resp(json(200, &rows)),
        _ => unavailable(),
    }
}

fn get_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    match ctx.backend.query(EngineQuery::Instance { instance: InstanceId(id.to_string()) }) {
        Ok(ReplyPayload::InstanceRow(Some(row))) => Outcome::Resp(json(200, &row)),
        Ok(ReplyPayload::InstanceRow(None)) => Outcome::Resp(message(404, "instance not found")),
        _ => unavailable(),
    }
}

fn delete_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    match ctx.backend.request(EngineCommand::DeleteInstance { instance: InstanceId(id.to_string()) }) {
        Ok(ReplyPayload::Deleted(true)) => Outcome::Resp(Resp::empty(204)),
        Ok(ReplyPayload::Deleted(false)) => Outcome::Resp(message(404, "instance not found")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

fn clear_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    match ctx.backend.request(EngineCommand::ClearInstance { instance: InstanceId(id.to_string()) }) {
        Ok(ReplyPayload::Cleared) => Outcome::Resp(Resp::empty(200)),
        Err(EngineError::NotFound(_)) => Outcome::Resp(message(404, "instance not found")),
        Err(EngineError::Conflict(Conflict::InstanceBusy)) => {
            Outcome::Resp(message(409, "instance has a run in flight; cancel it first"))
        }
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

/// `X-Flow-Actor` (DESIGN.md §3) is not on `makepad-bounded-http`'s tracked
/// header allowlist (shared with the asset store; out of this lane's
/// ownership), so the caller identity travels as `?actor=tab|chat|service`
/// instead — the same query-parameter workaround the store's own blob
/// upload route uses for metadata the transport does not carry.
fn put_instance_inputs(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, id: &str) -> Outcome {
    let actor = match head.query_get("actor") {
        Some("tab") => "tab",
        Some("chat") => "chat",
        _ => "service",
    }
    .to_string();
    let raw: HashMap<String, HashMap<String, crate::InputValueDto>> = match read_json(conn, head) {
        Ok(raw) => raw,
        Err(outcome) => return outcome,
    };
    match ctx.backend.set_instance_inputs(InstanceId(id.to_string()), raw, actor) {
        Ok(ReplyPayload::InputsSet(refs)) => Outcome::Resp(json(
            200,
            &crate::SetInputsResponse { inputs: refs.inputs, ordered_inputs: Some(refs.ordered_inputs) },
        )),
        Err(EngineError::NotFound(_)) => Outcome::Resp(message(404, "instance not found")),
        Err(EngineError::Conflict(Conflict::AskNotWaiting)) => Outcome::Resp(message(409, "Ask is not waiting there")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

// ---------------------------------------------------------------------------
// runs (§5.4, §6)
// ---------------------------------------------------------------------------

fn create_run(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, id: &str) -> Outcome {
    let request: CreateRunRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    match ctx.backend.start_run(InstanceId(id.to_string()), request.outputs) {
        Ok(ReplyPayload::RunStarted { run, queued }) => {
            Outcome::Resp(json(202, &CreateRunResponse { run_id: run.0, queued }))
        }
        Err(EngineError::NotFound(_)) => Outcome::Resp(message(404, "instance not found")),
        Err(EngineError::Conflict(Conflict::FlowInvalid)) => Outcome::Resp(message(409, "flow has no valid graph")),
        Err(EngineError::Conflict(Conflict::ConcurrencyZero)) => Outcome::Resp(message(409, "concurrency is zero")),
        // Every run lane is full, or the instance's run queue is.
        Err(EngineError::Busy(_)) => Outcome::Resp(message(503, "run capacity is exhausted")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

fn list_runs(head: &Head, ctx: &RouteCtx) -> Outcome {
    let instance = head.query_get("instance").map(|value| InstanceId(value.to_string()));
    match ctx.backend.query(EngineQuery::Runs { instance }) {
        Ok(ReplyPayload::Runs(rows)) => Outcome::Resp(json(200, &rows)),
        _ => unavailable(),
    }
}

fn get_run(ctx: &RouteCtx, id: &str) -> Outcome {
    match ctx.backend.query(EngineQuery::Run { run: RunId(id.to_string()) }) {
        Ok(ReplyPayload::RunRow(Some(row))) => Outcome::Resp(json(200, &row)),
        Ok(ReplyPayload::RunRow(None)) => Outcome::Resp(message(404, "run not found")),
        _ => unavailable(),
    }
}

fn cancel_run_route(ctx: &RouteCtx, id: &str) -> Outcome {
    match ctx.backend.cancel_run(RunId(id.to_string())) {
        Ok(ReplyPayload::Cancelled(true)) => Outcome::Resp(Resp::empty(200)),
        Ok(ReplyPayload::Cancelled(false)) => Outcome::Resp(message(404, "run not found")),
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}

// ---------------------------------------------------------------------------
// values (data plane, §5.5, §6)
// ---------------------------------------------------------------------------

fn get_value(head: &Head, ctx: &RouteCtx, digest_text: &str) -> Outcome {
    let Some(digest) = super::util::from_hex_32(digest_text) else {
        return Outcome::Resp(message(400, "malformed digest").closing());
    };
    let value = match ctx.backend.request(EngineCommand::ReadValue { digest }) {
        Ok(ReplyPayload::Value(value)) => value,
        _ => return unavailable(),
    };
    match value {
        Some(value) => Outcome::Resp(serve_value(head, &value, &digest)),
        None => Outcome::Resp(message(404, "value not found")),
    }
}

fn serve_value(head: &Head, value: &Value, digest: &[u8; 32]) -> Resp {
    let etag = format!("\"sha256:{}\"", super::util::to_hex(digest));
    if let Some(inm) = &head.if_none_match {
        if etag_matches(inm, &etag) {
            return Resp::empty(304)
                .with_header("ETag", etag)
                .with_header("Cache-Control", VALUE_CACHE_CONTROL.to_string());
        }
    }
    let size = value.bytes.len() as u64;
    let range = match &head.range {
        None => RangeSpec::None,
        Some(_) => {
            let honored = match &head.if_range {
                None => true,
                Some(if_range) => if_range_matches(if_range, &etag),
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
            return Resp::bytes(416, "text/plain; charset=utf-8", b"range not satisfiable".to_vec())
                .with_header("Content-Range", format!("bytes */{size}"))
                .with_header("ETag", etag);
        }
        RangeSpec::None => (200, &value.bytes[..], None),
        RangeSpec::Single { start, end } => (
            206,
            &value.bytes[start as usize..=end as usize],
            Some(format!("bytes {start}-{end}/{size}")),
        ),
    };
    let mut resp = Resp::bytes(status, &value.content_type, slice.to_vec())
        .with_header("ETag", etag)
        .with_header("Accept-Ranges", "bytes".to_string())
        .with_header("Cache-Control", VALUE_CACHE_CONTROL.to_string());
    if let Some(content_range) = content_range {
        resp = resp.with_header("Content-Range", content_range);
    }
    resp
}

/// `PUT /v1/values?type=<port type>&content_type=<mime>`, body = raw bytes
/// (media and registered job artifacts — text/json/list values are constructed inline from a
/// request's own literal, never uploaded). `content_type` (also a query
/// parameter, for the same reason `actor` is: the transport does not carry
/// `Content-Type` through to route handlers) defaults per `type` when
/// omitted.
fn put_value(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx) -> Outcome {
    let Some(ty) = head.query_get("type").and_then(PortType::from_str) else {
        return Outcome::Resp(message(400, "missing or invalid `type`").closing());
    };
    if !ty.is_media() && !matches!(ty, PortType::Registered(_)) {
        return Outcome::Resp(message(400, "text/json/list values must be supplied inline").closing());
    }
    if let Err(error) = ctx.config.registry.types().require_artifact(&ty) {
        return Outcome::Resp(message(400, &error.to_string()).closing());
    }
    let content_type = head
        .query_get("content_type")
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let bytes = match conn.read_body_full(head, MAX_VALUE_BYTES, BODY_DEADLINE_MS) {
        Ok(bytes) => bytes,
        Err(BodyError::Timeout) => return Outcome::Resp(message(408, "body timeout").closing()),
        Err(BodyError::Malformed) => return Outcome::Resp(message(400, "malformed body").closing()),
        Err(BodyError::TooLarge) => {
            conn.drain_remaining(head, Instant::now() + Duration::from_millis(BODY_DEADLINE_MS));
            return Outcome::Resp(message(413, "body too large").closing());
        }
        Err(BodyError::Io) => return Outcome::Hangup,
    };
    if bytes.is_empty() && ty.is_media() {
        return Outcome::Resp(message(400, "empty value").closing());
    }
    let value = match Value::artifact(ty, content_type, bytes, ctx.config.registry.types()) {
        Ok(value) => value,
        Err(error) => return Outcome::Resp(message(400, &error).closing()),
    };
    match ctx.backend.put_value(value) {
        Ok(ReplyPayload::ValueStored(digest)) => {
            Outcome::Resp(json(201, &PutValueResponse { digest: super::util::to_hex(&digest) }))
        }
        Err(error) => engine_error(error),
        Ok(_) => unavailable(),
    }
}
