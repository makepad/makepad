use super::config::SharedConfig;
use super::batches::CreateBatchOutcome;
use super::events::{EventCursor, EventHub};
use super::state::{
    ClearInstanceOutcome, CreateInstanceOutcome, SetInputsOutcome, SourceResult, StartRunOutcome,
    StateHandle,
};
use super::util::log;
use crate::{
    graph, CreateFromTemplateRequest, CreateInstanceRequest, CreateInstanceResponse,
    CreateBatchRequest, CreateRunRequest, CreateRunResponse, EvalErrorResponse, EventsResponse, FlowMutationResponse,
    FlowResponse, FlowSummary, HealthResponse, InstanceId, MessageResponse, NodesResponse,
    PortType, PutGraphRequest, PutSourceRequest, PutValueResponse, RevertRequest, RunId,
    AssetsResponse, TemplateResponse, Value,
};
use crate::engine::executors::publish::{AssetListQuery, AssetWorkerHandle};
use makepad_asset_data::AssetAlias;
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
    pub state: StateHandle,
    pub config: SharedConfig,
    pub server_id: [u8; 16],
    pub token: String,
    pub events: Arc<EventHub>,
    pub assets: AssetWorkerHandle,
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
        return call(&ctx.state, {
            let server_id = super::util::to_hex(&ctx.server_id);
            move |state| {
                json(
                    200,
                    &HealthResponse {
                        service: "makepad-flow".to_string(),
                        server_id,
                        protocol_version: 1,
                        revision_epoch: state.epoch,
                    },
                )
            }
        });
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
            if head.method != Method::Get {
                return Outcome::Resp(message(405, "method not allowed"));
            }
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
            match ctx.assets.list(AssetListQuery { text: q, namespace, limit }) {
                Ok(assets) => Outcome::Resp(json(200, &AssetsResponse { assets })),
                Err(error) => Outcome::Resp(message(
                    if error.contains("no asset server discovered") { 503 } else { 502 },
                    &error,
                )),
            }
        }
        [v1, assets, thumb, tail @ ..]
            if v1 == "v1" && assets == "assets" && thumb == "thumb" =>
        {
            if head.method != Method::Get {
                return Outcome::Resp(message(405, "method not allowed"));
            }
            let alias_text = tail.join("/");
            let alias = match AssetAlias::new(alias_text) {
                Ok(alias) => alias,
                Err(_) => return Outcome::Resp(message(400, "invalid asset alias")),
            };
            match ctx.assets.thumbnail(alias) {
                Ok(thumbnail) => Outcome::Resp(
                    Resp::bytes(200, &thumbnail.content_type, thumbnail.bytes)
                        .with_header("Cache-Control", "private, max-age=300".to_string()),
                ),
                Err(error) => Outcome::Resp(message(
                    if error.contains("not found") { 404 } else if error.contains("no asset server discovered") { 503 } else { 502 },
                    &error,
                )),
            }
        }
        [v1, health] if v1 == "v1" && health == "health" => {
            call(&ctx.state, |_| message(405, "method not allowed"))
        }
        [v1, nodes] if v1 == "v1" && nodes == "nodes" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            call(&ctx.state, |state| {
                let types = state.catalog_with_models();
                json(
                    200,
                    &NodesResponse {
                        types,
                        brief: crate::AUTHORING_BRIEF.to_string(),
                    },
                )
            })
        }
        [v1, models] if v1 == "v1" && models == "models" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            let domain = head.query_get("domain").map(str::to_string);
            call(&ctx.state, move |state| {
                let response = state.models_response(domain.as_deref());
                json(200, &response)
            })
        }
        [v1, templates] if v1 == "v1" && templates == "templates" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            let mut summaries: Vec<_> = crate::templates::TEMPLATES
                .iter()
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            let Some(template) = crate::templates::template(name) else {
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            call(&ctx.state, |state| {
                let rows: Vec<_> = state
                    .definitions
                    .values()
                    .map(|definition| {
                        let graph = definition.graph.as_ref();
                        let instances = state
                            .instances
                            .values()
                            .filter(|instance| instance.flow == definition.name)
                            .count() as u64;
                        FlowSummary {
                            name: definition.name.clone(),
                            label: graph
                                .map(|graph| graph.label.clone())
                                .unwrap_or_else(|| definition.name.clone()),
                            revision: definition.revision,
                            state: if definition.error.is_some() { "error" } else { "ok" }
                                .to_string(),
                            error: definition.error.clone(),
                            canonical: definition.canonical,
                            instances,
                            autostart: graph.is_some_and(|graph| graph.autostart),
                        }
                    })
                    .collect();
                json(200, &rows)
            })
        }
        [v1, events] if v1 == "v1" && events == "events" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                _ => call(&ctx.state, |_| message(405, "method not allowed")),
            }
        }
        [v1, flows, name, graph_segment]
            if v1 == "v1" && flows == "flows" && graph_segment == "graph" =>
        {
            if !valid_name(name) {
                return Outcome::Resp(message(400, "invalid flow name"));
            }
            if head.method != Method::Put {
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            cancel_batch_route(ctx, id)
        }
        [v1, batches, id] if v1 == "v1" && batches == "batches" => {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid batch id"));
            }
            if head.method != Method::Delete {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            clear_batch_route(ctx, id)
        }
        [v1, instances] if v1 == "v1" && instances == "instances" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                _ => call(&ctx.state, |_| message(405, "method not allowed")),
            }
        }
        [v1, instances, id, inputs]
            if v1 == "v1" && instances == "instances" && inputs == "inputs" =>
        {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid instance id"));
            }
            if head.method != Method::Put {
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            create_run(conn, head, ctx, id)
        }
        [v1, runs] if v1 == "v1" && runs == "runs" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            list_runs(head, ctx)
        }
        [v1, runs, id] if v1 == "v1" && runs == "runs" => {
            if !valid_id(id) {
                return Outcome::Resp(message(400, "invalid run id"));
            }
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
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
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            cancel_run_route(ctx, id)
        }
        _ => call(&ctx.state, |_| message(404, "not found")),
    }
}

/// Data plane: value bytes only (§5.5, §6). Instance/run/flow control-plane
/// routes never touch this plane.
fn data_dispatch(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx) -> Outcome {
    let segments = head.segs.clone();
    match segments.as_slice() {
        [v1, values, digest] if v1 == "v1" && values == "values" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            get_value(head, ctx, digest)
        }
        [v1, values] if v1 == "v1" && values == "values" => {
            if head.method != Method::Put {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            put_value(conn, head, ctx)
        }
        _ => call(&ctx.state, |_| message(404, "not found")),
    }
}

fn get_flow(ctx: &RouteCtx, name: &str) -> Outcome {
    let name = name.to_string();
    call(&ctx.state, move |state| {
        let Some(definition) = state.definitions.get(&name) else {
            return message(404, "flow not found");
        };
        json(
            200,
            &FlowResponse {
                source: definition.source.clone(),
                revision: definition.revision,
                graph: definition.graph.clone(),
                tools: definition.tools.clone(),
                error: definition.error.clone(),
            },
        )
    })
}

fn put_source(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: PutSourceRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    put_source_value(ctx, name.to_string(), request.source)
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
    let Some(template) = crate::templates::template(&request.template) else {
        return Outcome::Resp(message(404, "template not found"));
    };
    let name = name.to_string();
    let source = template.source.to_string();
    let config = ctx.config.clone();
    call(&ctx.state, move |state| match state.create_source(name, source) {
        Ok(Some(result)) => create_mutation_result(result),
        Ok(None) => message(409, "flow already exists"),
        Err(error) => {
            log(&config, &format!("flow create failed: {error}"));
            message(500, "internal server error")
        }
    })
}

fn put_graph(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: PutGraphRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let name = name.to_string();
    let config = ctx.config.clone();
    call(&ctx.state, move |state| {
        let source = graph::write(&request.graph);
        match state.put_source(name, source) {
            Ok(result) => mutation_result(result),
            Err(error) => {
                log(&config, &format!("flow graph write failed: {error}"));
                message(500, "internal server error")
            }
        }
    })
}

fn put_source_value(ctx: &RouteCtx, name: String, source: String) -> Outcome {
    let config = ctx.config.clone();
    call(&ctx.state, move |state| match state.put_source(name, source) {
        Ok(result) => mutation_result(result),
        Err(error) => {
            log(&config, &format!("flow write failed: {error}"));
            message(500, "internal server error")
        }
    })
}

fn revert_flow(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: RevertRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let name = name.to_string();
    let config = ctx.config.clone();
    call(&ctx.state, move |state| match state.revert(&name, request.revision) {
        Ok(Some(result)) => mutation_result(result),
        Ok(None) => message(404, "revision not found"),
        Err(error) => {
            log(&config, &format!("flow revert failed: {error}"));
            message(500, "internal server error")
        }
    })
}

fn delete_flow(ctx: &RouteCtx, name: &str) -> Outcome {
    let name = name.to_string();
    let config = ctx.config.clone();
    call(&ctx.state, move |state| match state.remove(&name) {
        Ok(true) => Resp::empty(204),
        Ok(false) => message(404, "flow not found"),
        Err(error) => {
            log(&config, &format!("flow delete failed: {error}"));
            message(500, "internal server error")
        }
    })
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
    if ctx.state.call(|_| ()).is_none() {
        return Outcome::Resp(message(503, "state unavailable"));
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

fn call(
    state: &StateHandle,
    closure: impl FnOnce(&mut super::state::FlowState) -> Resp + Send + 'static,
) -> Outcome {
    match state.call(closure) {
        Some(response) => Outcome::Resp(response),
        None => Outcome::Resp(message(503, "state unavailable")),
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
    let name = name.to_string();
    call(&ctx.state, move |state| {
        let inputs = request.inputs.unwrap_or_default();
        match state.create_instance(&name, request.label, request.pin.unwrap_or(false), inputs) {
            CreateInstanceOutcome::Created(id) => {
                json(201, &CreateInstanceResponse { instance: id.0 })
            }
            CreateInstanceOutcome::FlowNotFound => message(404, "flow not found"),
            CreateInstanceOutcome::FlowInvalid => message(409, "flow has no valid graph"),
            CreateInstanceOutcome::Error(error) => message(422, &error),
        }
    })
}

fn parallelism_route(ctx: &RouteCtx, name: &str) -> Outcome {
    let name = name.to_string();
    call(&ctx.state, move |state| match state.parallelism(&name) {
        Some(response) => json(200, &response),
        None if state.definitions.contains_key(&name) => message(409, "flow has no valid graph"),
        None => message(404, "flow not found"),
    })
}

fn create_batch(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, name: &str) -> Outcome {
    let request: CreateBatchRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let name = name.to_string();
    call(&ctx.state, move |state| match state.create_batch(&name, request) {
        CreateBatchOutcome::Created(response) => json(202, &response),
        CreateBatchOutcome::FlowNotFound => message(404, "flow not found"),
        CreateBatchOutcome::FlowInvalid => message(409, "flow has no valid graph"),
        CreateBatchOutcome::InvalidParallel => message(422, "parallel must be between 1 and 256"),
        CreateBatchOutcome::Error(error) => message(422, &error),
    })
}

fn cancel_batch_route(ctx: &RouteCtx, id: &str) -> Outcome {
    let id = id.to_string();
    call(&ctx.state, move |state| match state.cancel_batch(&id) {
        Some(runs) => json(200, &crate::BatchMutationResponse { runs }),
        None => message(404, "batch not found"),
    })
}

fn clear_batch_route(ctx: &RouteCtx, id: &str) -> Outcome {
    let id = id.to_string();
    call(&ctx.state, move |state| match state.clear_batch(&id) {
        Some(runs) => json(200, &crate::BatchMutationResponse { runs }),
        None => message(404, "batch not found"),
    })
}

fn list_instances(head: &Head, ctx: &RouteCtx) -> Outcome {
    let flow = head.query_get("flow").map(str::to_string);
    let waiting_only = head.query_get("waiting") == Some("1");
    call(&ctx.state, move |state| {
        json(200, &state.list_instance_rows(flow.as_deref(), waiting_only))
    })
}

fn get_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    let id = InstanceId(id.to_string());
    call(&ctx.state, move |state| match state.instance_row(&id) {
        Some(row) => json(200, &row),
        None => message(404, "instance not found"),
    })
}

fn delete_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    let id = InstanceId(id.to_string());
    call(&ctx.state, move |state| {
        if state.delete_instance(&id) {
            Resp::empty(204)
        } else {
            message(404, "instance not found")
        }
    })
}

fn clear_instance(ctx: &RouteCtx, id: &str) -> Outcome {
    let id = InstanceId(id.to_string());
    call(&ctx.state, move |state| match state.clear_instance(&id) {
        ClearInstanceOutcome::Cleared => Resp::empty(200),
        ClearInstanceOutcome::InstanceNotFound => message(404, "instance not found"),
        ClearInstanceOutcome::Busy => message(409, "instance has a run in flight; cancel it first"),
    })
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
    let id = InstanceId(id.to_string());
    call(&ctx.state, move |state| {
        match state.set_instance_inputs(&id, raw, &actor) {
            SetInputsOutcome::Ok(inputs) => json(200, &crate::SetInputsResponse { inputs }),
            SetInputsOutcome::InstanceNotFound => message(404, "instance not found"),
            SetInputsOutcome::AskNotWaiting => message(409, "Ask is not waiting there"),
            SetInputsOutcome::Error(error) => message(422, &error),
        }
    })
}

// ---------------------------------------------------------------------------
// runs (§5.4, §6)
// ---------------------------------------------------------------------------

fn create_run(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx, id: &str) -> Outcome {
    let request: CreateRunRequest = match read_json(conn, head) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let id = InstanceId(id.to_string());
    call(&ctx.state, move |state| match state.start_run(&id, request.outputs) {
        StartRunOutcome::Started { run_id, queued } => {
            json(202, &CreateRunResponse { run_id, queued })
        }
        StartRunOutcome::InstanceNotFound => message(404, "instance not found"),
        StartRunOutcome::FlowInvalid => message(409, "flow has no valid graph"),
        StartRunOutcome::Busy => message(409, "concurrency is zero"),
    })
}

fn list_runs(head: &Head, ctx: &RouteCtx) -> Outcome {
    let instance = head.query_get("instance").map(|value| InstanceId(value.to_string()));
    call(&ctx.state, move |state| {
        json(200, &state.list_run_rows(instance.as_ref()))
    })
}

fn get_run(ctx: &RouteCtx, id: &str) -> Outcome {
    let run_id = RunId(id.to_string());
    call(&ctx.state, move |state| match state.run_row(&run_id) {
        Some(row) => json(200, &row),
        None => message(404, "run not found"),
    })
}

fn cancel_run_route(ctx: &RouteCtx, id: &str) -> Outcome {
    let run_id = RunId(id.to_string());
    call(&ctx.state, move |state| {
        if state.cancel_run_and_retire_batch_instance(&run_id) {
            Resp::empty(200)
        } else {
            message(404, "run not found")
        }
    })
}

// ---------------------------------------------------------------------------
// values (data plane, §5.5, §6)
// ---------------------------------------------------------------------------

fn get_value(head: &Head, ctx: &RouteCtx, digest_text: &str) -> Outcome {
    let Some(digest) = super::util::from_hex_32(digest_text) else {
        return Outcome::Resp(message(400, "malformed digest").closing());
    };
    let value = match ctx.state.call(move |state| state.get_value(&digest)) {
        Some(value) => value,
        None => return Outcome::Resp(message(503, "state unavailable")),
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
/// (media only — text/json/list values are constructed inline from a
/// request's own literal, never uploaded). `content_type` (also a query
/// parameter, for the same reason `actor` is: the transport does not carry
/// `Content-Type` through to route handlers) defaults per `type` when
/// omitted.
fn put_value(conn: &mut Conn, head: &mut Head, ctx: &RouteCtx) -> Outcome {
    let Some(ty) = head.query_get("type").and_then(parse_media_port_type) else {
        return Outcome::Resp(message(400, "missing or non-media `type`").closing());
    };
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
    if bytes.is_empty() {
        return Outcome::Resp(message(400, "empty value").closing());
    }
    let value = Value::media(ty, content_type, bytes);
    call(&ctx.state, move |state| {
        let digest = state.put_value(value);
        json(201, &PutValueResponse { digest: super::util::to_hex(&digest) })
    })
}

fn parse_media_port_type(text: &str) -> Option<PortType> {
    let ty = match text {
        "image" => PortType::Image,
        "audio" => PortType::Audio,
        "video" => PortType::Video,
        "mesh" => PortType::Mesh,
        "bytes" => PortType::Bytes,
        _ => return None,
    };
    ty.is_media().then_some(ty)
}
