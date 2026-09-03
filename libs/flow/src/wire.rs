use makepad_micro_serde::*;

#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct Loc {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
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

/// One journal event. The common flow-definition fields are projected here;
/// newer event-specific fields remain safely ignorable to this lane.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct Event {
    pub seq: u64,
    pub topic: String,
    pub kind: String,
    pub name: Option<String>,
    pub revision: Option<u64>,
    pub canonical: Option<bool>,
    pub error: Option<EvalError>,
    pub instance: Option<String>,
    pub run_id: Option<String>,
}

/// Resumable long-poll page from `GET /v1/events`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct EventsPage {
    pub events: Vec<Event>,
    /// Opaque, epoch-stamped (`<16 hex>-<seq>`); echo it back verbatim.
    pub cursor: String,
    pub gap: bool,
}
