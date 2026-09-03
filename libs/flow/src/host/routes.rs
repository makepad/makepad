use super::config::SharedConfig;
use super::events::{EventCursor, EventHub};
use super::state::{SourceResult, StateHandle, MAX_SOURCE_BYTES};
use super::util::log;
use crate::{
    graph, EvalErrorResponse, EventsResponse, FlowMutationResponse, FlowResponse, FlowSummary,
    HealthResponse, MessageResponse, NodesResponse, PutGraphRequest, PutSourceRequest,
    RevertRequest,
};
use makepad_bounded_http::{BodyError, Conn, Head, Method, Resp};
use makepad_micro_serde::{DeJson, SerJson};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BODY_DEADLINE_MS: u64 = 30_000;
const EVENT_MAX_WAIT_MS: u64 = 30_000;
const EVENT_MAX_BATCH: usize = 256;

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
        return call(&ctx.state, |_| message(404, "not found"));
    }

    let segments = head.segs.clone();
    match segments.as_slice() {
        [v1, health] if v1 == "v1" && health == "health" => {
            call(&ctx.state, |_| message(405, "method not allowed"))
        }
        [v1, nodes] if v1 == "v1" && nodes == "nodes" => {
            if head.method != Method::Get {
                return call(&ctx.state, |_| message(405, "method not allowed"));
            }
            call(&ctx.state, |state| {
                json(
                    200,
                    &NodesResponse {
                        types: state.catalog.clone(),
                        brief: crate::AUTHORING_BRIEF.to_string(),
                    },
                )
            })
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
                            instances: 0,
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
    if request.source.len() as u64 > MAX_SOURCE_BYTES {
        return Outcome::Resp(message(413, "source too large").closing());
    }
    put_source_value(ctx, name.to_string(), request.source)
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
        if source.len() as u64 > MAX_SOURCE_BYTES {
            return message(413, "source too large").closing();
        }
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
    let bytes = match conn.read_body_full(head, MAX_SOURCE_BYTES, BODY_DEADLINE_MS) {
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
