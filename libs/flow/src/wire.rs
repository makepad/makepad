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

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.message)
    }
}

impl std::error::Error for EvalError {}
