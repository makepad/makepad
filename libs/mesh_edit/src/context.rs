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

/// Host-supplied working memory budget; unbounded unless the host supplies one.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: usize::MAX,
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
        Ok(())
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
    /// Admit working storage before a caller allocates it.
    pub fn bytes(&self, bytes: usize) -> Result<()> {
        self.limit("bytes", bytes as u64, self.limits.max_bytes as u64)
    }
    pub(crate) fn counts(
        &self,
        vertices: usize,
        faces: usize,
        corners: usize,
        edges: usize,
    ) -> Result<()> {
        // Mesh counts and corner offsets are u32 in the mesh source format.
        for (what, n) in [("vertices", vertices), ("faces", faces), ("corners", corners), ("edges", edges)] {
            self.limit(what, n as u64, u32::MAX as u64)?;
        }
        Ok(())
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> MeshError {
    MeshError::InvalidInput(message.into())
}
