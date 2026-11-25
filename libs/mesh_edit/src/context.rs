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

static PACE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// A host running mesh work on a cooperative pool installs its "give way if due"
/// function here. Returns true for the first installation in the process.
pub fn install_pace(pace: fn()) -> bool {
    PACE.set(pace).is_ok()
}

/// Give the host's cooperative pool a turn if due. The host may park the thread here, so call it
/// only where no lock is held that a job holding a CPU token could wait on: generators building
/// new data, not edits of shared state.
#[inline]
pub fn pace() {
    if let Some(pace) = PACE.get() { pace(); }
}

pub struct Context<'a> {
    pub limits: Limits,
    cancelled: Option<&'a dyn Fn() -> bool>,
    work: u64,
    /// Its polls give the host's pool a turn when due (`pacing`).
    pacing: bool,
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
            pacing: false,
        }
    }
    /// A context whose ops build new data and hold no lock that a job holding a CPU token could wait
    /// on (a generator's own mesh, not an edit of shared state): its polls give the host's
    /// cooperative pool a turn when due (`pace`), at an op's start and every 256 units of work.
    pub fn pacing(mut self) -> Self {
        self.pacing = true;
        self
    }
    pub fn work_used(&self) -> u64 {
        self.work
    }
    pub fn checkpoint(&mut self, work: u64) -> Result<()> {
        if self.cancelled.is_some_and(|f| f()) {
            return Err(MeshError::Cancelled);
        }
        let before = self.work;
        self.work = self.work.saturating_add(work);
        if self.pacing && (work == 0 || before >> 8 != self.work >> 8) {
            pace();
        }
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
