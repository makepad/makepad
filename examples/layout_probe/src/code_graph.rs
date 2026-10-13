//! Layout specimens from `apps/commercial/scope/deps/code_graph/src/`.
//! The `Graph` prefix separates these copies from synthetic specimens. Field
//! order, variant order, discriminants and repr attributes match the source;
//! methods and derives are omitted because only size and alignment are read.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

// graph.rs: arena IDs and interned symbols.
pub type GraphEntityId = u32;
pub type GraphEdgeId = u32;
pub type GraphSiteId = u32;
pub type GraphScopeId = u32;
pub type GraphCfgId = u32;
pub type GraphMetricsId = u32;
pub type GraphSym = u32;

#[repr(u8)]
pub enum GraphEntityKind {
    Workspace = 0,
    Group = 1,
    Package = 2,
    Target = 3,
    Module = 4,
    File = 5,
    Unattached = 6,
    Struct = 7,
    Enum = 8,
    Variant = 9,
    Union = 10,
    Trait = 11,
    Impl = 12,
    Function = 13,
    Closure = 14,
    Field = 15,
    TypeAlias = 16,
    Const = 17,
    Static = 18,
    Macro = 19,
    External = 20,
    Widget = 21,
    ExecutionContext = 22,
    Channel = 23,
    Lock = 24,
    Class = 25,
    Namespace = 26,
}

#[repr(u8)]
pub enum GraphEdgeKind {
    Contains = 0,
    Includes = 1,
    Uses = 2,
    DependsOn = 3,
    MacroCall = 4,
    UsesType = 5,
    FieldType = 6,
    Calls = 7,
    MethodCall = 8,
    Implements = 9,
    ForType = 10,
    Submits = 11,
    Sends = 12,
    Receives = 13,
    Locks = 14,
    Touches = 15,
    Declares = 16,
    Backs = 17,
    Handles = 18,
    Composes = 19,
    RunsIn = 20,
    Owns = 21,
    Takes = 22,
    Returns = 23,
    Inherits = 24,
}

pub enum GraphBasis {
    Exact,
    Bound,
    Derived { rule: GraphSym, version: u32 },
    Inferred { chain: Vec<GraphStep> },
    Candidates { n: u32 },
    Unknown,
}

pub struct GraphStep {
    pub rule: GraphSym,
    pub inputs: Vec<GraphStableKey>,
    pub sites: Vec<GraphSiteId>,
}

pub enum GraphNamespace {
    Type,
    Value,
    Macro,
}

pub enum GraphReceiver {
    Known(GraphEntityId),
    Param(GraphSym),
    Unknown,
}

#[repr(u8)]
pub enum GraphTypeRole {
    Field = 0,
    Bound = 1,
    Construct = 2,
    Pattern = 3,
    Cast = 4,
    Local = 5,
    Param = 6,
    Return = 7,
    Receiver = 8,
    AliasTarget = 9,
    ImplSelf = 10,
    ImplTrait = 11,
    Static = 12,
}

#[repr(u8)]
pub enum GraphOwnership {
    Inline = 0,
    Optional = 1,
    Boxed = 2,
    Collection = 3,
    Cell = 4,
    Shared = 5,
    Weak = 6,
    Borrowed = 7,
    BorrowedMut = 8,
    RawPointer = 9,
    Unclassified = 10,
}

pub enum GraphWidgetNamespace {
    Widget,
    Animator,
    Shader,
}

pub enum GraphDepKind {
    Normal,
    Dev,
    Build,
}

pub enum GraphSiteDetail {
    Structural,
    Path {
        written: GraphSym,
        namespace: GraphNamespace,
        position: GraphPathPosition,
    },
    Import {
        written: GraphSym,
        alias: Option<GraphSym>,
        glob: bool,
    },
    Dependency {
        kind: GraphDepKind,
        alias: GraphSym,
        features: Range<u32>,
        optional: bool,
    },
    Include {
        path: GraphSym,
    },
    Macro {
        path: GraphSym,
        body: GraphByteSpan,
    },
    MethodCall {
        name: GraphSym,
        receiver: GraphReceiver,
        candidates: Range<u32>,
        steps: Range<u32>,
        receiver_span: u32,
        binding: GraphSym,
    },
    TypeUse {
        role: GraphTypeRole,
        ownership: GraphOwnership,
        ordinal: u16,
        position: GraphSym,
        expr: GraphSym,
    },
    Derive {
        written: GraphSym,
    },
    Widget {
        path: GraphSym,
        namespace: GraphWidgetNamespace,
        root: GraphDeclKey,
    },
}

pub struct GraphSite {
    pub edge: Option<GraphEdgeId>,
    pub owner: GraphEntityId,
    pub scope: GraphScopeId,
    pub file: u32,
    pub span: Option<GraphByteSpan>,
    pub cfg: GraphCfgId,
    pub basis: GraphBasis,
    pub detail: GraphSiteDetail,
}

pub enum GraphVisibility {
    Private,
    Public,
    Crate,
    Restricted(GraphScopeId),
    Unresolved(GraphSym),
}

#[repr(u8)]
pub enum GraphBodyStatus {
    NotApplicable = 0,
    Present = 1,
    Absent = 2,
    Opaque = 3,
}

pub struct GraphEntity {
    pub key: GraphStableKey,
    pub decl: GraphDeclKey,
    pub stable_input: GraphStableInput,
    pub decl_input: GraphDeclInput,
    pub kind: GraphEntityKind,
    pub parent: Option<GraphEntityId>,
    pub unit: GraphEntityId,
    pub name: GraphSym,
    pub qualified: GraphSym,
    pub code_ref: Option<GraphCodeRef>,
    pub cfg: GraphCfgId,
    pub metrics: GraphMetricsId,
    pub visibility: GraphVisibility,
    pub flags: u16,
    pub body: GraphBodyStatus,
}

pub struct GraphEdge {
    pub from: GraphEntityId,
    pub to: GraphEntityId,
    pub kind: GraphEdgeKind,
    pub basis: GraphBasis,
    pub count: u64,
    pub sites: Range<GraphSiteId>,
}

pub struct GraphLookup {
    pub scope: GraphScopeId,
    pub namespace: GraphNamespace,
    pub name: GraphSym,
    pub table: GraphSym,
}

pub struct GraphUnresolved {
    pub site: Option<GraphSiteId>,
    pub path: GraphSym,
    pub scope: Option<GraphScopeId>,
    pub reason: GraphSym,
    pub candidates: Range<u32>,
    pub searched: Range<u32>,
}

pub enum GraphScopeKind {
    Module,
    Item,
    Block,
}

pub struct GraphScope {
    pub kind: GraphScopeKind,
    pub parent: Option<GraphScopeId>,
    pub owner: Option<GraphEntityId>,
    pub module: Option<GraphEntityId>,
    pub unit: GraphEntityId,
}

pub struct GraphMetrics {
    pub tokens: u64,
    pub lines: u64,
    pub items: u64,
    pub functions: u64,
    pub public_items: u64,
    pub complexity: u64,
    pub nesting: u64,
    pub unresolved: u64,
    pub fan_in: u64,
    pub fan_out: u64,
    pub consumers: u64,
    pub fan_in_inferred: u64,
    pub fan_out_inferred: u64,
    pub type_uses: u64,
}

// keys.rs
pub struct GraphStableKey(pub [u8; 20]);
pub struct GraphDeclKey(pub [u8; 20]);

pub struct GraphStableInput {
    pub kind: GraphEntityKind,
    pub target: GraphSym,
    pub module: GraphSym,
    pub name: GraphSym,
    pub disambiguator: GraphSym,
    pub cfg: GraphSym,
}

pub struct GraphDeclInput {
    pub target: GraphSym,
    pub path: GraphSym,
    pub kind: GraphEntityKind,
    pub ordinal: u32,
    pub header: GraphSym,
}

// source.rs
pub type GraphContentHash = [u8; 20];

pub struct GraphByteSpan {
    pub start: u64,
    pub end: u64,
}

pub struct GraphDocumentStamp {
    pub workspace_id: String,
    pub worktree_id: String,
    pub path: String,
    pub document_version: Option<u64>,
    pub content_hash: GraphContentHash,
    pub tree: Option<[u8; 20]>,
}

pub struct GraphSymbolRef {
    pub stable_id: GraphStableKey,
    pub qualified_name: String,
    pub disambiguator: String,
}

pub struct GraphCodeRef {
    pub document: Arc<GraphDocumentStamp>,
    pub bytes: Option<GraphByteSpan>,
    pub graph_revision: u64,
    pub analysis_context: Arc<GraphAnalysisContext>,
    pub symbol: Option<GraphSymbolRef>,
}

// cfg.rs
pub struct GraphAnalysisContext {
    pub target: String,
    pub features: BTreeMap<String, Vec<String>>,
    pub cfg: BTreeMap<String, Vec<String>>,
    pub complete_cfg_keys: Vec<String>,
    pub test: bool,
}

// parse/body.rs
pub enum GraphPathPosition {
    Expr,
    Call,
    StructLit,
    MacroName,
    Pattern,
}
