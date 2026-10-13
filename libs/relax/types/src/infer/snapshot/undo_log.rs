// ported from N:rustc_infer/src/infer/snapshot/undo_log.rs @8d1a764
use crate::infer::{InferCtxtInner, opaque_types, region_constraints, type_variable};

pub(crate) enum UndoLog {
    ConstUnificationTable(u32, Option<crate::infer::unify_key::ConstVar>),
    IntUnificationTable(u32, Option<crate::infer::unify_key::IntVar>),
    FloatUnificationTable(u32, Option<crate::infer::unify_key::FloatVar>),
    TypeVariables(type_variable::UndoLog),
    RegionConstraintCollector(region_constraints::UndoLog),
    OpaqueTypes(opaque_types::OpaqueTypeKey, Option<opaque_types::ProvisionalHiddenType>),
    DuplicateOpaqueType,
    PushRegionObligation,
}

pub(crate) struct Snapshot { undo_len: usize }

#[derive(Default)]
pub(crate) struct InferCtxtUndoLogs {
    logs: Vec<UndoLog>,
    num_open_snapshots: usize,
}

impl InferCtxtUndoLogs {
    pub(crate) fn push(&mut self, undo: UndoLog) {
        if self.in_snapshot() { self.logs.push(undo); }
    }
    pub(crate) fn in_snapshot(&self) -> bool { self.num_open_snapshots != 0 }
    pub(crate) fn num_open_snapshots(&self) -> usize { self.num_open_snapshots }
    pub(crate) fn start_snapshot(&mut self) -> Snapshot {
        self.num_open_snapshots += 1;
        Snapshot { undo_len: self.logs.len() }
    }
    pub(crate) fn opaque_types_in_snapshot(&self, snapshot: &Snapshot) -> bool {
        for log in &self.logs[snapshot.undo_len..] { if matches!(log, UndoLog::OpaqueTypes(..)) { return true; } }
        false
    }
    pub(crate) fn verify_in_snapshot(&self, snapshot: &Snapshot) -> bool {
        self.logs[snapshot.undo_len..].iter().any(|log| matches!(log, UndoLog::RegionConstraintCollector(region_constraints::UndoLog::AddVerify(_))))
    }
    pub(crate) fn region_constraints_in_snapshot(&self, snapshot: &Snapshot) -> bool {
        for log in &self.logs[snapshot.undo_len..] {
            if matches!(log, UndoLog::RegionConstraintCollector(region_constraints::UndoLog::AddConstraint(_))) { return true; }
        }
        false
    }
}

impl InferCtxtInner {
    fn reverse(&mut self, undo: UndoLog) {
        match undo {
            UndoLog::ConstUnificationTable(vid, old) => self.const_unification_storage.reverse(vid, old),
            UndoLog::IntUnificationTable(vid, old) => self.int_unification_storage.reverse(vid, old),
            UndoLog::FloatUnificationTable(vid, old) => self.float_unification_storage.reverse(vid, old),
            UndoLog::TypeVariables(undo) => self.type_variable_storage.reverse(undo),
            UndoLog::RegionConstraintCollector(undo) => self.region_constraint_storage.as_mut().unwrap().reverse(undo),
            UndoLog::OpaqueTypes(key, previous) => self.opaque_type_storage.remove(key, previous),
            UndoLog::DuplicateOpaqueType => self.opaque_type_storage.pop_duplicate_entry(),
            UndoLog::PushRegionObligation => { self.region_obligations.pop().unwrap(); }
        }
    }
    pub(crate) fn rollback_to(&mut self, snapshot: Snapshot) {
        assert!(self.undo_log.num_open_snapshots > 0);
        assert!(self.undo_log.logs.len() >= snapshot.undo_len);
        while self.undo_log.logs.len() > snapshot.undo_len {
            let undo = self.undo_log.logs.pop().unwrap();
            self.reverse(undo);
        }
        if self.undo_log.num_open_snapshots == 1 { assert_eq!(snapshot.undo_len, 0); }
        self.undo_log.num_open_snapshots -= 1;
    }
    pub(crate) fn commit(&mut self, snapshot: Snapshot) {
        assert!(self.undo_log.num_open_snapshots > 0);
        if self.undo_log.num_open_snapshots == 1 {
            assert_eq!(snapshot.undo_len, 0);
            self.undo_log.logs.clear();
        }
        self.undo_log.num_open_snapshots -= 1;
    }
}
