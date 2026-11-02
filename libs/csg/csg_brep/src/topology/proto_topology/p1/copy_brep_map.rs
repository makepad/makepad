//! `SmCopyBrepMap`: which topology object of an original B-rep each object of its copy
//! was copied from.

use std::collections::BTreeMap;

use crate::error::{Result, SolidError};

/// A map from the topology objects of a copied B-rep to those of the original
/// (`SmCopyBrepMap`). Objects are caller-chosen handles `O`; `None` is a null pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyBrepMap<O> {
    /// `m_pOrigBrep`.
    orig_brep: Option<O>,
    /// `m_pCopyBrep`.
    copy_brep: Option<O>,
    /// `m_sMap`: copy object to original object.
    map: BTreeMap<O, O>,
}

impl<O> Default for CopyBrepMap<O> {
    fn default() -> Self {
        Self {
            orig_brep: None,
            copy_brep: None,
            map: BTreeMap::new(),
        }
    }
}

impl<O: Copy + Ord> CopyBrepMap<O> {
    /// An empty map without B-reps (`SmCopyBrepMap()`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The destructor `~SmCopyBrepMap`: forgets both B-reps (the map is dropped).
    pub fn destroy(&mut self) {
        self.orig_brep = None;
        self.copy_brep = None;
    }

    /// Sets the original and copied B-reps (`SetBreps`).
    pub fn set_breps(&mut self, orig_brep: Option<O>, copy_brep: Option<O>) {
        self.orig_brep = orig_brep;
        self.copy_brep = copy_brep;
    }

    /// The original B-rep (`GetOrigBrep`).
    #[must_use]
    pub const fn orig_brep(&self) -> Option<O> {
        self.orig_brep
    }

    /// The copied B-rep (`GetCopyBrep`).
    #[must_use]
    pub const fn copy_brep(&self) -> Option<O> {
        self.copy_brep
    }

    /// Records that `copy_obj` is the copy of `orig_obj`, replacing any earlier entry
    /// for `copy_obj` (`SetAt`). Both objects are required: upstream's `SM_ASSERT`s
    /// still evaluate `GetType()` of both in the release build.
    pub fn set_at(&mut self, copy_obj: O, orig_obj: O) {
        self.map.insert(copy_obj, orig_obj);
    }

    /// The original of `copy_obj` (`GetAt`): `None` for a null object.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an object without an entry: upstream's release
    /// `SM_ASSERT` calls `GetType()` on the null original it finds (undefined).
    pub fn get_at(&self, copy_obj: Option<O>) -> Result<Option<O>> {
        let Some(c) = copy_obj else {
            return Ok(None);
        };
        self.map
            .get(&c)
            .copied()
            .map(Some)
            .ok_or(SolidError::InvalidInput {
                operation: "topology::proto_topology::p1::CopyBrepMap::get_at",
                reason: "object has no entry (upstream dereferences the null original)",
            })
    }

    /// The number of entries (`m_sMap.Count()`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when the map has no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
