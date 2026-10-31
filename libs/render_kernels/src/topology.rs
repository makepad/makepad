//! What a kernel buffer is drawn as, and the validation a publish runs.
//! Emitted index buffers are checked before they are published: every index
//! must be below the published vertex count (the platform checks ordinary
//! geometry the same way, `geometry.rs`).

/// A kernel geometry's primitive topology (`GeometryRef::Kernel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    /// Triangle list; `indexed` takes an index buffer (three per triangle).
    Triangles { indexed: bool },
    /// Line list; `indexed` takes an index buffer (two per line).
    Lines { indexed: bool },
    Points,
    /// One record per instance of the draw's own geometry.
    Instances,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// An index at or past the vertex count (the first one found).
    IndexOutOfRange { at: usize, index: u32, count: u32 },
    /// The index (or vertex) count is not whole primitives.
    Ragged { len: usize, per: usize },
    /// Indices given to a non-indexed topology.
    UnexpectedIndices,
}

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyError::IndexOutOfRange { at, index, count } => write!(f, "index {} at {} is not below the vertex count {}", index, at, count),
            TopologyError::Ragged { len, per } => write!(f, "{} is not a whole number of primitives of {}", len, per),
            TopologyError::UnexpectedIndices => write!(f, "indices given to a topology without an index buffer"),
        }
    }
}

impl Topology {
    /// Vertices (or indices) per primitive.
    pub fn per_primitive(self) -> usize {
        match self {
            Topology::Triangles { .. } => 3,
            Topology::Lines { .. } => 2,
            Topology::Points | Topology::Instances => 1,
        }
    }

    pub fn indexed(self) -> bool {
        matches!(self, Topology::Triangles { indexed: true } | Topology::Lines { indexed: true })
    }

    /// Checks `count` records with `indices` (empty when not indexed).
    pub fn validate(self, count: u32, indices: &[u32]) -> Result<(), TopologyError> {
        let per = self.per_primitive();
        if !self.indexed() {
            if !indices.is_empty() {
                return Err(TopologyError::UnexpectedIndices);
            }
            if count as usize % per != 0 {
                return Err(TopologyError::Ragged { len: count as usize, per });
            }
            return Ok(());
        }
        if indices.len() % per != 0 {
            return Err(TopologyError::Ragged { len: indices.len(), per });
        }
        // The common case is one pass with no branch per index: the max.
        let max = indices.iter().copied().max().unwrap_or(0);
        if !indices.is_empty() && max >= count {
            let at = indices.iter().position(|&i| i >= count).unwrap_or(0);
            return Err(TopologyError::IndexOutOfRange { at, index: indices[at], count });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_must_stay_below_the_vertex_count() {
        let t = Topology::Triangles { indexed: true };
        assert_eq!(t.validate(3, &[0, 1, 2]), Ok(()));
        assert_eq!(t.validate(3, &[0, 1, 3]), Err(TopologyError::IndexOutOfRange { at: 2, index: 3, count: 3 }));
        assert_eq!(t.validate(3, &[0, 1]), Err(TopologyError::Ragged { len: 2, per: 3 }));
        assert_eq!(t.validate(0, &[]), Ok(()));
        let l = Topology::Lines { indexed: false };
        assert_eq!(l.validate(4, &[]), Ok(()));
        assert_eq!(l.validate(3, &[]), Err(TopologyError::Ragged { len: 3, per: 2 }));
        assert_eq!(Topology::Points.validate(3, &[1]), Err(TopologyError::UnexpectedIndices));
        assert_eq!(Topology::Instances.validate(7, &[]), Ok(()));
    }
}
