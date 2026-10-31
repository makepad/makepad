// ported from N:rustc_infer/src/infer/snapshot/mod.rs @8d1a764
pub(crate) mod undo_log;
mod fudge;
pub use fudge::VariableLengths;
use super::InferCtxt;
use super::region_constraints::RegionSnapshot;
use crate::types::UniverseIndex;

#[must_use = "snapshots must be committed or rolled back"]
pub struct CombinedSnapshot {
    pub(crate) undo_snapshot: undo_log::Snapshot,
    region_constraints_snapshot: RegionSnapshot,
    pub(crate) region_constraints_len: usize,
    universe: UniverseIndex,
}

impl InferCtxt<'_> {
    pub fn in_snapshot(&self) -> bool { self.inner.borrow().undo_log.in_snapshot() }
    pub fn num_open_snapshots(&self) -> usize { self.inner.borrow().undo_log.num_open_snapshots() }

    pub(crate) fn start_snapshot(&self) -> CombinedSnapshot {
        let mut inner = self.inner.borrow_mut();
        CombinedSnapshot {
            undo_snapshot: inner.undo_log.start_snapshot(),
            region_constraints_snapshot: inner.unwrap_region_constraints().start_snapshot(),
            region_constraints_len: inner.unwrap_region_constraints().constraints().len(),
            universe: self.universe(),
        }
    }
    pub(crate) fn rollback_to(&self, snapshot: CombinedSnapshot) {
        self.universe.set(snapshot.universe);
        let mut inner = self.inner.borrow_mut();
        inner.rollback_to(snapshot.undo_snapshot);
        inner.unwrap_region_constraints().rollback_to(snapshot.region_constraints_snapshot);
    }
    pub(crate) fn commit_from(&self, snapshot: CombinedSnapshot) { self.inner.borrow_mut().commit(snapshot.undo_snapshot); }

    pub fn commit_if_ok<T, E>(&self, f: impl FnOnce(&CombinedSnapshot) -> Result<T, E>) -> Result<T, E> {
        let snapshot = self.start_snapshot();
        let result = f(&snapshot);
        if result.is_ok() { self.commit_from(snapshot); } else { self.rollback_to(snapshot); }
        result
    }
    pub fn probe<T>(&self, f: impl FnOnce(&CombinedSnapshot) -> T) -> T {
        let snapshot = self.start_snapshot();
        let result = f(&snapshot);
        self.rollback_to(snapshot);
        result
    }
    pub fn region_constraints_added_in_snapshot(&self, snapshot: &CombinedSnapshot) -> bool {
        self.inner.borrow().undo_log.region_constraints_in_snapshot(&snapshot.undo_snapshot)
    }
    pub fn opaque_types_added_in_snapshot(&self, snapshot: &CombinedSnapshot) -> bool {
        self.inner.borrow().undo_log.opaque_types_in_snapshot(&snapshot.undo_snapshot)
    }
}
