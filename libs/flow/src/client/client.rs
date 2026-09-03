use super::http::{HttpClient, Method};
use crate::{
    CreateFromTemplateRequest, CreateInstanceRequest, CreateInstanceResponse, CreateRunResponse,
    EvalError, EventsPage, FlowDefinition, FlowSummary, Graph, Health, InstanceRow, NodesResponse,
    ModelsResponse, PutFlowResponse, RunRowDto, SetInputsResponse, TemplateResponse,
    TemplateSummary, ValueBytes,
};
use makepad_micro_serde::{DeJson, SerJson};
use makepad_strict_json::Value;
use std::net::SocketAddr;
use std::time::Duration;

pub const CONTROL_BODY_CAP: usize = 1024 * 1024;
pub const DATA_BODY_CAP: usize = 64 * 1024 * 1024;
pub const PROTOCOL_VERSION: u16 = 1;

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    Io {
        op: &'static str,
        kind: std::io::ErrorKind,
    },
    Timeout(String),
    Http {
        status: u16,
        body: String,
    },
    Protocol(String),
    Unauthorized,
    ServerIdentityMismatch,
    Eval(EvalError),
}

impl ClientError {
    pub fn is_connection_loss(&self) -> bool {
        matches!(self, Self::Io { .. } | Self::Timeout(_))
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { op, kind } => write!(f, "I/O failure during {op}: {kind:?}"),
            Self::Timeout(op) => write!(f, "timeout during {op}"),
            Self::Http { status, body } if body.is_empty() => {
                write!(f, "HTTP request failed with status {status}")
            }
            Self::Http { status, body } => {
                write!(f, "HTTP request failed with status {status}: {body}")
            }
            Self::Protocol(message) => write!(f, "protocol violation: {message}"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::ServerIdentityMismatch => f.write_str("server identity mismatch"),
            Self::Eval(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub control: SocketAddr,
    pub data: SocketAddr,
}

/// Verified blocking client for one flow server.
pub struct FlowClient {
    endpoints: Endpoints,
    token: String,
    server_id: [u8; 16],
    control: HttpClient,
    data: HttpClient,
}

impl std::fmt::Debug for FlowClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowClient")
            .field("endpoints", &self.endpoints)
            .field("server_id", &hex(&self.server_id))
            .finish_non_exhaustive()
    }
}

impl FlowClient {
    pub fn connect(
        endpoints: Endpoints,
        token: String,
        expected_server_id: Option<[u8; 16]>,
    ) -> ClientResult<Self> {
        let client = Self::unverified(endpoints, token, [0; 16]);
        let health = client.health()?;
        if let Some(expected) = expected_server_id {
            if health.server_id != expected {
                return Err(ClientError::ServerIdentityMismatch);
            }
        }
        if health.protocol_version != PROTOCOL_VERSION {
            return Err(ClientError::Protocol(format!(
                "unsupported protocol version {}",
                health.protocol_version
            )));
        }
        let client = Self {
            server_id: health.server_id,
            ..client
        };
        // Authentication is part of connection establishment, not a later
        // surprise on the first user action.
        let _ = client.flows()?;
        Ok(client)
    }

    fn unverified(endpoints: Endpoints, token: String, server_id: [u8; 16]) -> Self {
        Self {
            endpoints,
            token,
            server_id,
            control: HttpClient::new(endpoints.control, CONTROL_BODY_CAP),
            data: HttpClient::new(endpoints.data, DATA_BODY_CAP),
        }
    }

    pub fn endpoints(&self) -> Endpoints {
        self.endpoints
    }

    pub fn server_id(&self) -> [u8; 16] {
        self.server_id
    }

    pub(crate) fn subscription_lane(&self) -> Self {
        Self::unverified(self.endpoints, self.token.clone(), self.server_id)
    }

    pub fn health(&self) -> ClientResult<Health> {
        let body = self.call(Method::Get, "/v1/health", None, false, None)?;
        parse_health(&body)
    }

    pub fn nodes(&self) -> ClientResult<Value> {
        let body = self.call(Method::Get, "/v1/nodes", None, true, None)?;
        parse_json(&body)
    }

    pub fn templates(&self) -> ClientResult<Vec<TemplateSummary>> {
        let body = self.call(Method::Get, "/v1/templates", None, true, None)?;
        decode(&body, "template list")
    }

    pub fn template(&self, name: &str) -> ClientResult<TemplateResponse> {
        let target = format!("/v1/templates/{}", flow_name(name)?);
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "template response")
    }

    /// The hub fleet's live models, optionally for one domain (`image`,
    /// `video`, …). Additive for flow-ui's model picker (lane F8b serves it).
    pub fn nodes_catalog(&self) -> ClientResult<NodesResponse> {
        let body = self.call(Method::Get, "/v1/nodes", None, true, None)?;
        decode(&body, "node catalog")
    }

    pub fn models(&self, domain: Option<&str>) -> ClientResult<ModelsResponse> {
        let target = match domain {
            Some(domain) => format!("/v1/models?domain={}", model_domain(domain)?),
            None => "/v1/models".to_string(),
        };
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "model list")
    }

    pub fn flows(&self) -> ClientResult<Vec<FlowSummary>> {
        let body = self.call(Method::Get, "/v1/flows", None, true, None)?;
        decode(&body, "flow list")
    }

    pub fn flow(&self, name: &str) -> ClientResult<FlowDefinition> {
        let target = format!("/v1/flows/{}", flow_name(name)?);
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "flow definition")
    }

    pub fn put_source(&self, name: &str, source: &str) -> ClientResult<PutFlowResponse> {
        let target = format!("/v1/flows/{}", flow_name(name)?);
        let body = Value::Obj(vec![("source".into(), Value::Str(source.into()))])
            .to_json()
            .into_bytes();
        let response = self.call(Method::Put, &target, Some(&body), true, None)?;
        decode(&response, "put source response")
    }

    pub fn create_from_template(
        &self,
        name: &str,
        template: &str,
    ) -> ClientResult<PutFlowResponse> {
        let target = format!("/v1/flows/{}", flow_name(name)?);
        let body = CreateFromTemplateRequest { template: template.to_string() }
            .serialize_json()
            .into_bytes();
        let response = self.call(Method::Post, &target, Some(&body), true, None)?;
        decode(&response, "create from template response")
    }

    pub fn put_graph(&self, name: &str, graph: &Graph) -> ClientResult<PutFlowResponse> {
        let target = format!("/v1/flows/{}/graph", flow_name(name)?);
        let body = format!("{{\"graph\":{}}}", graph.serialize_json()).into_bytes();
        let response = self.call(Method::Put, &target, Some(&body), true, None)?;
        decode(&response, "put graph response")
    }

    pub fn revert(&self, name: &str, revision: u64) -> ClientResult<PutFlowResponse> {
        let target = format!("/v1/flows/{}/revert", flow_name(name)?);
        let body = format!("{{\"revision\":{revision}}}").into_bytes();
        let response = self.call(Method::Post, &target, Some(&body), true, None)?;
        decode(&response, "revert response")
    }

    pub fn delete(&self, name: &str) -> ClientResult<()> {
        let target = format!("/v1/flows/{}", flow_name(name)?);
        let _ = self.call(Method::Delete, &target, None, true, None)?;
        Ok(())
    }

    pub fn instances(
        &self,
        flow: Option<&str>,
        waiting: bool,
    ) -> ClientResult<Vec<InstanceRow>> {
        let target = instances_target(flow, waiting)?;
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "instance list")
    }

    pub fn instances_json(&self, flow: Option<&str>, waiting: bool) -> ClientResult<Value> {
        let target = instances_target(flow, waiting)?;
        self.call_json(Method::Get, &target, None)
    }

    pub fn instance(&self, id: &str) -> ClientResult<InstanceRow> {
        let target = format!("/v1/instances/{}", route_id(id, "instance")?);
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "instance row")
    }

    pub fn instance_json(&self, id: &str) -> ClientResult<Value> {
        let target = format!("/v1/instances/{}", route_id(id, "instance")?);
        self.call_json(Method::Get, &target, None)
    }

    pub fn create_instance(
        &self,
        name: &str,
        request: &CreateInstanceRequest,
    ) -> ClientResult<CreateInstanceResponse> {
        let target = format!("/v1/flows/{}/instances", flow_name(name)?);
        let body = request.serialize_json().into_bytes();
        let response = self.call(Method::Post, &target, Some(&body), true, None)?;
        decode(&response, "create instance response")
    }

    pub fn create_instance_json(&self, name: &str, request: &Value) -> ClientResult<Value> {
        let target = format!("/v1/flows/{}/instances", flow_name(name)?);
        let body = request.to_json().into_bytes();
        self.call_json(Method::Post, &target, Some(&body))
    }

    pub fn put_inputs(
        &self,
        id: &str,
        actor: &str,
        inputs: &Value,
    ) -> ClientResult<SetInputsResponse> {
        let target = instance_inputs_target(id, actor)?;
        let body = inputs.to_json().into_bytes();
        let response = self.call(Method::Put, &target, Some(&body), true, None)?;
        decode(&response, "set inputs response")
    }

    pub fn put_inputs_json(&self, id: &str, actor: &str, inputs: &Value) -> ClientResult<Value> {
        let target = instance_inputs_target(id, actor)?;
        let body = inputs.to_json().into_bytes();
        self.call_json(Method::Put, &target, Some(&body))
    }

    pub fn start_run(
        &self,
        id: &str,
        outputs: Option<&[String]>,
    ) -> ClientResult<CreateRunResponse> {
        let target = format!("/v1/instances/{}/runs", route_id(id, "instance")?);
        let body = Value::Obj(match outputs {
            Some(outputs) => vec![(
                "outputs".to_string(),
                Value::Arr(outputs.iter().cloned().map(Value::Str).collect()),
            )],
            None => Vec::new(),
        })
        .to_json()
        .into_bytes();
        let response = self.call(Method::Post, &target, Some(&body), true, None)?;
        decode(&response, "start run response")
    }

    pub fn start_run_json(&self, id: &str, outputs: Option<&[String]>) -> ClientResult<Value> {
        let target = format!("/v1/instances/{}/runs", route_id(id, "instance")?);
        let body = Value::Obj(match outputs {
            Some(outputs) => vec![(
                "outputs".to_string(),
                Value::Arr(outputs.iter().cloned().map(Value::Str).collect()),
            )],
            None => Vec::new(),
        })
        .to_json()
        .into_bytes();
        self.call_json(Method::Post, &target, Some(&body))
    }

    pub fn run(&self, id: &str) -> ClientResult<RunRowDto> {
        let target = format!("/v1/runs/{}", route_id(id, "run")?);
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "run row")
    }

    pub fn runs(&self, instance: Option<&str>) -> ClientResult<Vec<RunRowDto>> {
        let target = match instance {
            Some(instance) => format!(
                "/v1/runs?instance={}",
                route_id(instance, "instance")?
            ),
            None => "/v1/runs".to_string(),
        };
        let body = self.call(Method::Get, &target, None, true, None)?;
        decode(&body, "run list")
    }

    pub fn run_json(&self, id: &str) -> ClientResult<Value> {
        let target = format!("/v1/runs/{}", route_id(id, "run")?);
        self.call_json(Method::Get, &target, None)
    }

    pub fn cancel_run(&self, id: &str) -> ClientResult<()> {
        let target = format!("/v1/runs/{}/cancel", route_id(id, "run")?);
        let _ = self.call(Method::Post, &target, None, true, None)?;
        Ok(())
    }

    pub fn clear_instance(&self, id: &str) -> ClientResult<()> {
        let target = format!("/v1/instances/{}/clear", route_id(id, "instance")?);
        let _ = self.call(Method::Post, &target, None, true, None)?;
        Ok(())
    }

    pub fn delete_instance(&self, id: &str) -> ClientResult<()> {
        let target = format!("/v1/instances/{}", route_id(id, "instance")?);
        let _ = self.call(Method::Delete, &target, None, true, None)?;
        Ok(())
    }

    pub fn value(&self, digest: &str) -> ClientResult<ValueBytes> {
        let target = format!("/v1/values/{}", value_digest(digest)?);
        let response = self
            .data
            .call(Method::Get, &target, Some(self.token.as_str()), None, None)?;
        match response.status {
            200..=299 => Ok(ValueBytes {
                digest: digest.to_string(),
                content_type: response
                    .content_type
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                bytes: response.body.into(),
            }),
            401 => Err(ClientError::Unauthorized),
            status => Err(http_error(status, &response.body)),
        }
    }

    pub fn events(
        &self,
        cursor: Option<&str>,
        wait_ms: u64,
        limit: u32,
        topic: Option<&str>,
    ) -> ClientResult<EventsPage> {
        if wait_ms > 30_000 {
            return Err(ClientError::Protocol("event wait exceeds 30000 ms".into()));
        }
        if limit == 0 || limit > 4096 {
            return Err(ClientError::Protocol("event limit is out of range".into()));
        }
        let mut target = format!("/v1/events?wait={wait_ms}&limit={limit}");
        if let Some(cursor) = cursor {
            target.push_str("&cursor=");
            target.push_str(&cursor.to_string());
        }
        if let Some(topic) = topic {
            if topic.is_empty()
                || !topic
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                return Err(ClientError::Protocol("invalid event topic".into()));
            }
            target.push_str("&topic=");
            target.push_str(topic);
        }
        let head_deadline = Duration::from_millis(wait_ms.saturating_add(2_000));
        let body = self.call(
            Method::Get,
            &target,
            None,
            true,
            Some(head_deadline),
        )?;
        decode(&body, "events response")
    }

    fn call(
        &self,
        method: Method,
        target: &str,
        body: Option<&[u8]>,
        authenticated: bool,
        head_deadline: Option<Duration>,
    ) -> ClientResult<Vec<u8>> {
        let response = self.control.call(
            method,
            target,
            authenticated.then_some(self.token.as_str()),
            body,
            head_deadline,
        )?;
        match response.status {
            200..=299 => Ok(response.body),
            401 => Err(ClientError::Unauthorized),
            422 => match parse_eval_response(&response.body) {
                Some(error) => Err(ClientError::Eval(error)),
                None => Err(http_error(response.status, &response.body)),
            },
            status => Err(http_error(status, &response.body)),
        }
    }

    fn call_json(
        &self,
        method: Method,
        target: &str,
        body: Option<&[u8]>,
    ) -> ClientResult<Value> {
        let response = self.call(method, target, body, true, None)?;
        parse_json(&response)
    }
}

fn parse_health(bytes: &[u8]) -> ClientResult<Health> {
    let value = parse_json(bytes)?;
    let service = field(&value, "service")?
        .as_str()
        .ok_or_else(|| ClientError::Protocol("health service is not a string".into()))?;
    if service != "makepad-flow" {
        return Err(ClientError::Protocol("health service is not makepad-flow".into()));
    }
    let server_id = parse_server_id(field(&value, "server_id")?)
        .ok_or_else(|| ClientError::Protocol("health server_id is malformed".into()))?;
    let protocol_version = field(&value, "protocol_version")?
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| ClientError::Protocol("health protocol_version is malformed".into()))?;
    let revision_epoch = field(&value, "revision_epoch")?
        .as_u64()
        .ok_or_else(|| ClientError::Protocol("health revision_epoch is malformed".into()))?;
    Ok(Health {
        service: service.into(),
        server_id,
        protocol_version,
        revision_epoch,
    })
}

fn parse_server_id(value: &Value) -> Option<[u8; 16]> {
    if let Some(text) = value.as_str() {
        return parse_hex16(text);
    }
    let values = value.as_arr()?;
    if values.len() != 16 {
        return None;
    }
    let mut out = [0u8; 16];
    for (index, value) in values.iter().enumerate() {
        out[index] = u8::try_from(value.as_u64()?).ok()?;
    }
    Some(out)
}

fn parse_eval_response(bytes: &[u8]) -> Option<EvalError> {
    let root = makepad_strict_json::parse(bytes).ok()?;
    let error = root.get("error")?;
    Some(EvalError {
        file: error
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        line: u32::try_from(error.get("line")?.as_u64()?).ok()?,
        col: u32::try_from(error.get("col")?.as_u64()?).ok()?,
        message: error.get("message")?.as_str()?.to_string(),
    })
}

fn decode<T: DeJson>(bytes: &[u8], what: &str) -> ClientResult<T> {
    let _ = parse_json(bytes)?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ClientError::Protocol(format!("{what} is not UTF-8")))?;
    // Syntax/framing was already checked by `makepad-strict-json`; the
    // typed projection stays additive across server releases by ignoring
    // fields this client version does not know yet.
    T::deserialize_json_lenient(text)
        .map_err(|error| ClientError::Protocol(format!("malformed {what}: {error:?}")))
}

/// A graph nests deeper than the strict parser's default cap of 8 (definition →
/// graph → nodes → inputs → value → edge, plus literal objects), so typed
/// bodies are checked at 32; the byte cap still bounds the work.
const MAX_JSON_DEPTH: u32 = 32;

fn parse_json(bytes: &[u8]) -> ClientResult<Value> {
    makepad_strict_json::parse_depth(bytes, MAX_JSON_DEPTH)
        .map_err(|message| ClientError::Protocol(format!("malformed JSON: {message}")))
}

fn field<'a>(value: &'a Value, name: &str) -> ClientResult<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| ClientError::Protocol(format!("missing JSON field {name}")))
}

fn http_error(status: u16, bytes: &[u8]) -> ClientError {
    let mut body = String::from_utf8_lossy(bytes).into_owned();
    if body.len() > 16 * 1024 {
        body.truncate(16 * 1024);
    }
    body.retain(|character| character == '\n' || character == '\t' || !character.is_control());
    ClientError::Http { status, body }
}

fn flow_name(name: &str) -> ClientResult<&str> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || name == "."
        || name == ".."
    {
        return Err(ClientError::Protocol("invalid flow name".into()));
    }
    Ok(name)
}

fn model_domain(domain: &str) -> ClientResult<&str> {
    if domain.is_empty()
        || domain.len() > 64
        || !domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(ClientError::Protocol("invalid model domain".into()));
    }
    Ok(domain)
}

fn instances_target(flow: Option<&str>, waiting: bool) -> ClientResult<String> {
    let mut target = "/v1/instances".to_string();
    let mut separator = '?';
    if let Some(flow) = flow {
        target.push(separator);
        separator = '&';
        target.push_str("flow=");
        target.push_str(flow_name(flow)?);
    }
    if waiting {
        target.push(separator);
        target.push_str("waiting=1");
    }
    Ok(target)
}

fn instance_inputs_target(id: &str, actor: &str) -> ClientResult<String> {
    if !matches!(actor, "tab" | "chat" | "service") {
        return Err(ClientError::Protocol("invalid input actor".into()));
    }
    Ok(format!(
        "/v1/instances/{}/inputs?actor={actor}",
        route_id(id, "instance")?
    ))
}

fn route_id<'a>(id: &'a str, what: &str) -> ClientResult<&'a str> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(ClientError::Protocol(format!("invalid {what} id")));
    }
    Ok(id)
}

fn value_digest(digest: &str) -> ClientResult<&str> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ClientError::Protocol("invalid value digest".into()));
    }
    Ok(digest)
}

pub(crate) fn parse_hex16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

pub(crate) fn health_answers(endpoints: Endpoints) -> bool {
    let client = HttpClient::with_limits(
        endpoints.control,
        CONTROL_BODY_CAP,
        super::http::HttpLimits::probe(),
    );
    let Ok(response) = client.call(Method::Get, "/v1/health", None, None, None) else {
        return false;
    };
    response.status == 200 && parse_health(&response.body).is_ok()
}
