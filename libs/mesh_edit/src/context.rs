use crate::{ElementId, ValidationKind};
use std::fmt;

pub type Result<T> = std::result::Result<T, MeshError>;

#[derive(Clone, Debug, PartialEq)]
pub enum MeshError {
    Cancelled,
    Budget {
        resource: &'static str,
        requested: u64,
        limit: u64,
    },
    InvalidInput(String),
    UnknownElement(ElementId),
    InvalidGeometry {
        face: crate::FaceId,
        kind: ValidationKind,
    },
    CorruptData(&'static str),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "mesh operation cancelled"),
            Self::Budget {
                resource,
                requested,
                limit,
            } => write!(f, "{resource} budget exceeded: {requested} > {limit}"),
            Self::InvalidInput(s) => write!(f, "{s}"),
            Self::UnknownElement(id) => write!(f, "unknown element {id:?}"),
            Self::InvalidGeometry { face, kind } => write!(f, "invalid face {face:?}: {kind:?}"),
            Self::CorruptData(s) => write!(f, "corrupt mesh: {s}"),
        }
    }
}
impl std::error::Error for MeshError {}

/// Provisional worker limits, not claims about device performance. `max_bytes`
/// bounds conservative working storage estimates as well as serialized input.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_vertices: usize,
    pub max_faces: usize,
    pub max_corners: usize,
    pub max_edges: usize,
    pub max_face_corners: usize,
    pub max_triangles: usize,
    pub max_weights_per_vertex: usize,
    pub max_bytes: usize,
    pub max_work: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_vertices: 100_000,
            max_faces: 50_000,
            max_corners: 200_000,
            max_edges: 300_000,
            max_face_corners: 4096,
            max_triangles: 200_000,
            max_weights_per_vertex: 64,
            max_bytes: 128 * 1024 * 1024,
            max_work: 100_000_000,
        }
    }
}

pub struct Context<'a> {
    pub limits: Limits,
    cancelled: Option<&'a dyn Fn() -> bool>,
    work: u64,
}
pub type OpContext<'a> = Context<'a>;
impl Default for Context<'_> {
    fn default() -> Self {
        Self::new(Limits::default(), None)
    }
}
impl<'a> Context<'a> {
    pub fn new(limits: Limits, cancelled: Option<&'a dyn Fn() -> bool>) -> Self {
        Self {
            limits,
            cancelled,
            work: 0,
        }
    }
    pub fn work_used(&self) -> u64 {
        self.work
    }
    pub fn checkpoint(&mut self, work: u64) -> Result<()> {
        if self.cancelled.is_some_and(|f| f()) {
            return Err(MeshError::Cancelled);
        }
        self.work = self.work.saturating_add(work);
        self.limit("work", self.work, self.limits.max_work)
    }
    pub(crate) fn limit(&self, resource: &'static str, requested: u64, limit: u64) -> Result<()> {
        if requested > limit {
            Err(MeshError::Budget {
                resource,
                requested,
                limit,
            })
        } else {
            Ok(())
        }
    }
    pub(crate) fn bytes(&self, bytes: usize) -> Result<()> {
        self.limit("bytes", bytes as u64, self.limits.max_bytes as u64)
    }
    pub(crate) fn counts(
        &self,
        vertices: usize,
        faces: usize,
        corners: usize,
        edges: usize,
    ) -> Result<()> {
        for (what, n, max) in [
            ("vertices", vertices, self.limits.max_vertices),
            ("faces", faces, self.limits.max_faces),
            ("corners", corners, self.limits.max_corners),
            ("edges", edges, self.limits.max_edges),
        ] {
            self.limit(what, n as u64, max.min(u32::MAX as usize) as u64)?;
        }
        Ok(())
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> MeshError {
    MeshError::InvalidInput(message.into())
}
