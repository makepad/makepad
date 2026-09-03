use makepad_micro_serde::*;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct Loc {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortType {
    Text,
    Image,
    Audio,
    Video,
    Mesh,
    Json,
    List,
    Bytes,
}

impl PortType {
    pub fn is_media(self) -> bool {
        matches!(
            self,
            Self::Image | Self::Audio | Self::Video | Self::Mesh | Self::Bytes
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Mesh => "mesh",
            Self::Json => "json",
            Self::List => "list",
            Self::Bytes => "bytes",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value.to_ascii_lowercase().as_str() {
            "text" => Self::Text,
            "image" => Self::Image,
            "audio" => Self::Audio,
            "video" => Self::Video,
            "mesh" => Self::Mesh,
            "json" => Self::Json,
            "list" => Self::List,
            "bytes" => Self::Bytes,
            _ => return None,
        })
    }
}

// Port types are protocol strings, not micro_serde's default
// `{ "Text": [] }` representation for fieldless enums.
impl SerJson for PortType {
    fn ser_json(&self, depth: usize, state: &mut SerJsonState) {
        self.as_str().to_string().ser_json(depth, state);
    }
}

impl DeJson for PortType {
    fn de_json(
        state: &mut DeJsonState,
        input: &mut std::str::Chars,
    ) -> Result<Self, DeJsonErr> {
        let value = String::de_json(state, input)?;
        Self::from_str(&value).ok_or_else(|| state.err_enum("port type"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum RunState {
    Queued,
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum NodeState {
    Pending,
    Ready,
    Running,
    Waiting,
    Done,
    Failed,
    Skipped,
    Cancelled,
}

/// Content-addressed value metadata used by route/event DTOs. The bytes travel
/// on the value plane and deliberately are not embedded in JSON events.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ValueRef {
    #[rename(type)]
    pub ty: PortType,
    pub content_type: String,
    pub digest: String,
    pub bytes: usize,
    pub preview: Option<Literal>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct PortValueRef {
    pub port: String,
    pub value: ValueRef,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum RunEventPayload {
    RunStarted {
        run_id: String,
        instance: String,
        flow: String,
        revision: u64,
        planned_nodes: Vec<String>,
    },
    NodeStarted {
        node: String,
    },
    NodeProgress {
        node: String,
        permille: u16,
        stage: String,
    },
    NodeDelta {
        node: String,
        port: String,
        text: String,
    },
    NodeWaiting {
        node: String,
        question: String,
        ty: PortType,
        options: Vec<Literal>,
    },
    NodeAnswered {
        node: String,
        by: String,
    },
    NodeDone {
        node: String,
        outputs: Vec<PortValueRef>,
    },
    NodeFailed {
        node: String,
        error: String,
    },
    NodeSkipped {
        node: String,
        reason: String,
    },
    RunFinished {
        state: RunState,
        secs: f64,
        outputs: Vec<(String, ValueRef)>,
        http_log: Vec<HttpLogEntryDto>,
        warnings: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HttpLogEntryDto {
    pub ms: u64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
}

impl From<&crate::Value> for ValueRef {
    fn from(value: &crate::Value) -> Self {
        Self {
            ty: value.ty,
            content_type: value.content_type.clone(),
            digest: value.digest_hex(),
            bytes: value.bytes.len(),
            preview: value_preview(value),
        }
    }
}

fn value_preview(value: &crate::Value) -> Option<Literal> {
    if matches!(value.ty, PortType::Text | PortType::Json | PortType::List) {
        return Some(Literal::Str(
            String::from_utf8_lossy(&value.bytes)
                .chars()
                .take(512)
                .collect(),
        ));
    }
    if value.ty == PortType::Image
        && value.content_type.eq_ignore_ascii_case("image/png")
        && value.bytes.len() >= 24
        && &value.bytes[..8] == b"\x89PNG\r\n\x1a\n"
    {
        let width = u32::from_be_bytes(value.bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(value.bytes[20..24].try_into().unwrap());
        return Some(Literal::Obj(vec![
            ("width".to_string(), Literal::Num(width as f64)),
            ("height".to_string(), Literal::Num(height as f64)),
        ]));
    }
    None
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum Literal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Id(String),
    Arr(Vec<Literal>),
    Obj(Vec<(String, Literal)>),
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Port {
    pub name: String,
    pub ty: PortType,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum NodeInputValue {
    Literal(Literal),
    Edge(EdgeRef),
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct EdgeRef {
    pub from_node: String,
    pub from_port: String,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct NodeInput {
    pub port: String,
    pub ty: PortType,
    pub value: NodeInputValue,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Edge {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Node {
    pub id: String,
    pub kind: String,
    pub type_name: String,
    pub params: Vec<(String, Literal)>,
    pub inputs: Vec<NodeInput>,
    pub outputs: Vec<Port>,
    pub at: Option<(f64, f64)>,
    pub size: Option<(f64, f64)>,
    pub loc: Loc,
    pub fn_src: Option<String>,
    pub face_src: Option<String>,
    pub on_fail: String,
    pub label: Option<String>,
    pub domain: Option<String>,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolEntry {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Graph {
    pub revision: u64,
    pub label: String,
    pub brief: String,
    pub trigger: String,
    pub concurrency: u64,
    pub autostart: bool,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub tools: Vec<ToolEntry>,
    pub flow_ui_src: Option<String>,
    /// Non-fatal notes surfaced with the evaluated graph, for example a
    /// node still using the deprecated `ports: { in: [...] }` array form.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: String,
    pub result_fields: Vec<(String, PortType)>,
}

#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct ToolSchema {
    pub tools: Vec<ToolDef>,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct EvalError {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

// ---------------------------------------------------------------------------
// flow-server control-plane DTOs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HealthResponse {
    pub service: String,
    pub server_id: String,
    pub protocol_version: u32,
    pub revision_epoch: u64,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ParamRange {
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct NodeParamCatalog {
    pub name: String,
    pub default: JsonValue,
    pub doc: String,
    pub range: Option<ParamRange>,
}

#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct NodePortsCatalog {
    /// Leading underscore is stripped by micro-serde, yielding JSON `in`.
    pub _in: Vec<Port>,
    pub out: Vec<Port>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct NodeTypeCatalog {
    pub type_name: String,
    pub kind: String,
    pub domain: Option<String>,
    pub models: Vec<String>,
    pub ports: NodePortsCatalog,
    pub params: Vec<NodeParamCatalog>,
    pub face: String,
    pub doc: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct NodesResponse {
    pub types: Vec<NodeTypeCatalog>,
    pub brief: String,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct FleetNodeDto {
    pub base_url: String,
    pub fleet: String,
    pub healthy: bool,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ModelInfoDto {
    pub id: String,
    pub domain: String,
    pub backend: String,
    pub node: String,
    pub available: bool,
    pub gated: bool,
    pub state: String,
    pub vram_gb: Option<f64>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ModelsResponse {
    pub nodes: Vec<FleetNodeDto>,
    pub models: Vec<ModelInfoDto>,
    pub snapshot_ms: u64,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct TemplateSummary {
    pub name: String,
    pub label: String,
    pub brief: String,
    pub node_count: u64,
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct TemplateResponse {
    pub name: String,
    pub label: String,
    pub brief: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CreateFromTemplateRequest {
    pub template: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct FlowResponse {
    pub source: String,
    pub revision: u64,
    pub graph: Option<Graph>,
    pub tools: ToolSchema,
    pub error: Option<EvalError>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct PutSourceRequest {
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct PutGraphRequest {
    pub graph: Graph,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct RevertRequest {
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct FlowMutationResponse {
    pub revision: u64,
    pub graph: Graph,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct EvalErrorResponse {
    pub error: EvalError,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct MessageResponse {
    pub error: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct EventsResponse {
    pub events: Vec<JsonValue>,
    pub cursor: String,
    pub gap: bool,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.message)
    }
}

impl std::error::Error for EvalError {}

/// The identity handshake returned by `GET /v1/health` after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub service: String,
    pub server_id: [u8; 16],
    pub protocol_version: u16,
    pub revision_epoch: u64,
}

/// One row from `GET /v1/flows`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct FlowSummary {
    pub name: String,
    pub label: String,
    pub revision: u64,
    pub state: String,
    pub error: Option<EvalError>,
    pub canonical: bool,
    pub instances: u64,
    pub autostart: bool,
}

/// A definition, its last evaluated graph, and its callable tools.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct FlowDefinition {
    pub source: String,
    pub revision: u64,
    /// The last good graph; absent when the definition has never evaluated.
    pub graph: Option<Graph>,
    pub tools: ToolSchema,
    pub error: Option<EvalError>,
}

/// Success body shared by source, graph, and revert writes.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct PutFlowResponse {
    pub revision: u64,
    pub graph: Graph,
}

/// One journal event. The common flow-definition fields are projected here
/// and the run-event fields of §5.4 ride along as optional fields; anything
/// whose shape differs between kinds (`error` is an object on `flow.error`
/// and a string on `node.failed`; `outputs` is `[{port, value}]` on
/// `node.done` and `[[name, value]]` on `run.finished`) stays dynamic.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct Event {
    pub seq: u64,
    pub topic: String,
    pub kind: String,
    pub name: Option<String>,
    pub revision: Option<u64>,
    pub canonical: Option<bool>,
    pub error: Option<JsonValue>,
    pub instance: Option<String>,
    pub run_id: Option<String>,
    pub flow: Option<String>,
    pub node: Option<String>,
    pub port: Option<String>,
    pub text: Option<String>,
    pub permille: Option<u64>,
    pub stage: Option<String>,
    pub state: Option<JsonValue>,
    pub secs: Option<f64>,
    pub by: Option<String>,
    pub reason: Option<String>,
    pub question: Option<String>,
    pub outputs: Option<JsonValue>,
    /// Exact node-pruned execution set, present on `run.started`.
    pub planned_nodes: Option<Vec<String>>,
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq && self.topic == other.topic && self.kind == other.kind
    }
}

impl Event {
    /// `flow.error`'s structured error, when the event carries one.
    pub fn eval_error(&self) -> Option<EvalError> {
        let error = self.error.as_ref()?.object()?;
        Some(EvalError {
            file: error.get("file").and_then(json_text).unwrap_or_default(),
            line: error.get("line").and_then(json_u64)? as u32,
            col: error.get("col").and_then(json_u64)? as u32,
            message: error.get("message").and_then(json_text)?,
        })
    }

    /// The error as one line of text, whichever shape it came in.
    pub fn error_text(&self) -> Option<String> {
        let error = self.error.as_ref()?;
        json_text(error).or_else(|| self.eval_error().map(|error| error.to_string()))
    }

    /// The run or node state name as text (`done`, `failed`, …), lower-cased.
    pub fn state_text(&self) -> Option<String> {
        let state = self.state.as_ref()?;
        match state {
            JsonValue::Object(fields) => fields.keys().next().map(|key| key.to_lowercase()),
            other => json_text(other).map(|text| text.to_lowercase()),
        }
    }

    /// `node.done`'s outputs (`[{port, value}]`), or `run.finished`'s
    /// (`[[name, value]]`), as port/value pairs.
    pub fn output_values(&self) -> Vec<(String, ValueRef)> {
        let mut out = Vec::new();
        let Some(JsonValue::Array(items)) = self.outputs.as_ref() else {
            return out;
        };
        for item in items {
            match item {
                JsonValue::Object(fields) => {
                    let (Some(port), Some(value)) = (
                        fields.get("port").and_then(json_text),
                        fields.get("value").and_then(value_ref_from_json),
                    ) else {
                        continue;
                    };
                    out.push((port, value));
                }
                JsonValue::Array(pair) if pair.len() == 2 => {
                    let (Some(port), Some(value)) =
                        (json_text(&pair[0]), value_ref_from_json(&pair[1]))
                    else {
                        continue;
                    };
                    out.push((port, value));
                }
                _ => {}
            }
        }
        out
    }
}

pub fn json_u64(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::U64(value) => Some(*value),
        JsonValue::I64(value) => u64::try_from(*value).ok(),
        JsonValue::F64(value) if *value >= 0.0 => Some(*value as u64),
        _ => None,
    }
}

/// A JSON scalar or value reference rendered as text for event/UI helpers.
pub fn json_text(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(text) | JsonValue::BareIdent(text) => Some(text.clone()),
        JsonValue::Char(value) => Some(value.to_string()),
        JsonValue::Bool(value) => Some(value.to_string()),
        JsonValue::U64(value) => Some(value.to_string()),
        JsonValue::I64(value) => Some(value.to_string()),
        JsonValue::F64(value) => Some(value.to_string()),
        JsonValue::Object(fields) => fields
            .get("preview")
            .and_then(json_text)
            .or_else(|| fields.get("text").and_then(json_text)),
        _ => None,
    }
}

/// A `ValueRef` read from a dynamic JSON object.
pub fn value_ref_from_json(value: &JsonValue) -> Option<ValueRef> {
    let fields = value.object()?;
    let ty = match fields.get("type").and_then(json_text)?.to_lowercase().as_str() {
        "text" => PortType::Text,
        "image" => PortType::Image,
        "audio" => PortType::Audio,
        "video" => PortType::Video,
        "mesh" => PortType::Mesh,
        "json" => PortType::Json,
        "list" => PortType::List,
        "bytes" => PortType::Bytes,
        _ => return None,
    };
    Some(ValueRef {
        ty,
        content_type: fields
            .get("content_type")
            .and_then(json_text)
            .unwrap_or_default(),
        digest: fields.get("digest").and_then(json_text)?,
        bytes: fields.get("bytes").and_then(json_u64).unwrap_or(0) as usize,
        preview: fields.get("preview").and_then(literal_from_json),
    })
}

fn literal_from_json(value: &JsonValue) -> Option<Literal> {
    // Wire DTOs currently encode `Literal` through micro_serde's tagged enum
    // shape; face-authored/direct JSON values are accepted as well.
    if let Ok(literal) = Literal::deserialize_json(&value.serialize_json()) {
        return Some(literal);
    }
    Some(match value {
        JsonValue::Null | JsonValue::Undefined => Literal::Null,
        JsonValue::Bool(value) => Literal::Bool(*value),
        JsonValue::U64(value) => Literal::Num(*value as f64),
        JsonValue::U128(value) => Literal::Num(*value as f64),
        JsonValue::I64(value) => Literal::Num(*value as f64),
        JsonValue::I128(value) => Literal::Num(*value as f64),
        JsonValue::F64(value) => Literal::Num(*value),
        JsonValue::String(value) | JsonValue::BareIdent(value) => Literal::Str(value.clone()),
        JsonValue::Char(value) => Literal::Str(value.to_string()),
        JsonValue::Array(values) => {
            Literal::Arr(values.iter().filter_map(literal_from_json).collect())
        }
        JsonValue::Object(fields) => {
            let mut pairs: Vec<(String, Literal)> = fields
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), literal_from_json(value)?)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Literal::Obj(pairs)
        }
    })
}

/// Resumable long-poll page from `GET /v1/events`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct EventsPage {
    pub events: Vec<Event>,
    /// Opaque, epoch-stamped (`<16 hex>-<seq>`); echo it back verbatim.
    pub cursor: String,
    pub gap: bool,
}

// ---------------------------------------------------------------------------
// flow-server instance & run DTOs (§4.1, §5.4, §6)
// ---------------------------------------------------------------------------

/// One input value in a request body: `{type, text?, json?, digest?}`.
/// Exactly one of `text` / `json` / `digest` is populated, selected by
/// `ty` — `digest` references a value already in the store (an upload
/// through `PUT /v1/values`, or a prior run's output).
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct InputValueDto {
    #[rename(type)]
    pub ty: PortType,
    pub text: Option<String>,
    pub json: Option<JsonValue>,
    pub digest: Option<String>,
}

/// `POST /v1/flows/{name}/instances {label?, inputs?, pin?}`.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct CreateInstanceRequest {
    pub label: Option<String>,
    pub inputs: Option<HashMap<String, HashMap<String, InputValueDto>>>,
    pub pin: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CreateInstanceResponse {
    pub instance: String,
}

/// The question an instance is parked on, mirroring `RunEventPayload::NodeWaiting`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct WaitingDto {
    pub node: String,
    pub question: String,
    #[rename(type)]
    pub ty: PortType,
    pub options: Vec<Literal>,
}

/// One row from `GET /v1/instances` / `GET /v1/instances/{id}`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct InstanceRow {
    pub instance: String,
    pub flow: String,
    pub label: Option<String>,
    pub revision: u64,
    pub live: bool,
    pub state: String,
    pub run: Option<String>,
    pub inputs: HashMap<String, HashMap<String, ValueRef>>,
    pub outputs: HashMap<String, ValueRef>,
    pub waiting: Option<WaitingDto>,
    pub owner: String,
    pub created_ms: u64,
    pub last_activity_ms: u64,
    pub subscribers: u64,
}

impl InstanceRow {
    pub fn input_text(&self, node: &str, port: &str) -> Option<String> {
        literal_text(self.inputs.get(node)?.get(port)?.preview.as_ref()?)
    }
}

fn literal_text(value: &Literal) -> Option<String> {
    match value {
        Literal::Null => None,
        Literal::Bool(value) => Some(value.to_string()),
        Literal::Num(value) if value.fract() == 0.0 => Some((*value as i64).to_string()),
        Literal::Num(value) => Some(value.to_string()),
        Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
        Literal::Arr(_) | Literal::Obj(_) => Some(value.serialize_json()),
    }
}

/// `PUT /v1/instances/{id}/inputs` succeeds with the instance's inputs as
/// they stand after the write.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct SetInputsResponse {
    pub inputs: HashMap<String, HashMap<String, ValueRef>>,
}

/// `POST /v1/instances/{id}/runs {outputs?}`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct CreateRunRequest {
    pub outputs: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub queued: u64,
}

/// One node's row inside a run, keyed by node id in `RunRowDto::nodes`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct NodeRowDto {
    pub state: NodeState,
    pub progress: Option<u16>,
    pub stage: Option<String>,
    pub outputs: Vec<PortValueRef>,
    pub error: Option<String>,
    /// Delta text accumulated so far (streaming nodes), capped at 16 KiB.
    pub text: Option<String>,
}

/// `GET /v1/runs/{id}` and each row of `GET /v1/runs?instance=`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct RunRowDto {
    pub run_id: String,
    pub instance: String,
    pub flow: String,
    pub revision: u64,
    pub state: RunState,
    /// Exact node-pruned execution set used as the progress denominator.
    pub planned_nodes: Vec<String>,
    pub nodes: HashMap<String, NodeRowDto>,
    pub outputs: HashMap<String, ValueRef>,
    pub http_log: Vec<HttpLogEntryDto>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
}

/// `PUT /v1/values` (data plane, raw bytes body) succeeds with the
/// content-addressed digest, matching `ValueRef::digest`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct PutValueResponse {
    pub digest: String,
}

/// One value fetched over the data plane (`GET /v1/values/{digest}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueBytes {
    pub digest: String,
    pub content_type: String,
    pub bytes: std::sync::Arc<[u8]>,
}
