//! Data-only architecture plans, schema v0. Files live in the repository-root `arch/` tree, mirroring the source tree: `arch/<crate path>/crate.toml` and `arch/<crate path>/<module path>.toml`.
//! Filesystem operations are synchronous: call them on a worker, not the UI.
mod decode;
mod files;
mod format;
mod guard;

pub use decode::parse;
pub use files::{changes_since, manifest, try_manifest, validate};
pub use format::format;

use std::{collections::BTreeMap, path::PathBuf};

pub const SCHEMA: &str = include_str!("../SCHEMA.toml");
pub const PROMPT: &str = include_str!("../PROMPT.txt");
pub const EXAMPLE: &str = include_str!("../EXAMPLE.toml");
pub type NodeId = String;
pub type LaneId = String;
pub type EdgeId = String;

/// All limits can be tightened. Values above the defaults are clamped to the
/// defaults so an untrusted caller cannot disable the v0 resource bounds.
/// Prose lengths count Unicode scalar values; other string limits count bytes.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Applies to both the supplied text and its canonical representation.
    pub max_input_bytes: usize,
    pub max_depth: usize,
    /// Syntactic values plus key segments (including implicit tables).
    pub max_values: usize,
    /// Decoded string body / bare-token bytes, checked before allocation.
    pub max_string_bytes: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_lanes: usize,
    pub max_files: usize,
    pub max_overview_chars: usize,
    pub max_summary_chars: usize,
    pub max_story_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 256 * 1024,
            max_depth: 16,
            max_values: 8192,
            max_string_bytes: 16 * 1024,
            max_nodes: 24,
            max_edges: 48,
            max_lanes: 8,
            max_files: 512,
            max_overview_chars: 1200,
            max_summary_chars: 120,
            max_story_chars: 600,
        }
    }
}

impl Limits {
    pub(crate) fn bounded(&self) -> Self {
        let cap = Self::default();
        Self {
            max_input_bytes: self.max_input_bytes.min(cap.max_input_bytes),
            max_depth: self.max_depth.min(cap.max_depth),
            max_values: self.max_values.min(cap.max_values),
            max_string_bytes: self.max_string_bytes.min(cap.max_string_bytes),
            max_nodes: self.max_nodes.min(cap.max_nodes),
            max_edges: self.max_edges.min(cap.max_edges),
            max_lanes: self.max_lanes.min(cap.max_lanes),
            max_files: self.max_files.min(cap.max_files),
            max_overview_chars: self.max_overview_chars.min(cap.max_overview_chars),
            max_summary_chars: self.max_summary_chars.min(cap.max_summary_chars),
            max_story_chars: self.max_story_chars.min(cap.max_story_chars),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub arch: Header,
    pub source: Source,
    /// Serialized as `source.overview`, as in the plan of record's TOML.
    pub overview: String,
    pub lanes: BTreeMap<LaneId, Lane>,
    pub nodes: BTreeMap<NodeId, Node>,
    pub edges: BTreeMap<EdgeId, Edge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub scope: String,
    pub title: String,
    pub prompt: String,
    pub generator: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    pub files: Vec<FileHash>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHash {
    pub path: PathBuf,
    /// Lowercase, 40-digit hexadecimal Git blob content hash.
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub title: String,
    pub summary: String,
    pub story: String,
    pub lane: Option<LaneId>,
    pub refs: Vec<String>,
    pub budget: Option<String>,
    pub child: Option<String>,
    /// Opaque text: never parsed, evaluated, or interpreted in v0.
    pub visual: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub label: Option<String>,
}

macro_rules! kinds {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }
        impl std::str::FromStr for $name {
            type Err = ();
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($text => Ok(Self::$variant)),+, _ => Err(()) }
            }
        }
    };
}

kinds!(NodeKind {
    Component => "component", Thread => "thread", Queue => "queue",
    Store => "store", Memory => "memory", Gpu => "gpu", Io => "io",
});
kinds!(EdgeKind {
    Owns => "owns", Spawns => "spawns", Sends => "sends", Reads => "reads",
    Writes => "writes", AllocatesFrom => "allocates_from",
    UploadsTo => "uploads_to", Calls => "calls",
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax,
    Limit,
    UnknownField,
    MissingField,
    Type,
    Version,
    InvalidKind,
    Duplicate,
    Dangling,
    InvalidId,
    InvalidPath,
    InvalidRef,
    InvalidHash,
    MissingPath,
    Io,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchError {
    pub kind: ErrorKind,
    /// Dotted schema field or repository-relative path, where available.
    pub field: String,
    pub message: String,
    pub offset: Option<usize>,
}

impl ArchError {
    pub(crate) fn new(kind: ErrorKind, field: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            field: field.into(),
            message: message.into(),
            offset: None,
        }
    }
}

impl std::fmt::Display for ArchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)?;
        if let Some(offset) = self.offset {
            write!(f, " at byte {offset}")?;
        }
        Ok(())
    }
}
impl std::error::Error for ArchError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validation {
    pub errors: Vec<ArchError>,
    pub warnings: Vec<Warning>,
}
impl Validation {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub affected_nodes: Vec<NodeId>,
    /// False on any IO, containment, or discovery error, even without a delta.
    pub unchanged: bool,
    pub errors: Vec<ArchError>,
}

pub(crate) fn limit(field: &str, actual: usize, max: usize) -> Result<(), ArchError> {
    if actual > max {
        Err(ArchError::new(
            ErrorKind::Limit,
            field,
            format!("limit {max} exceeded ({actual})"),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
