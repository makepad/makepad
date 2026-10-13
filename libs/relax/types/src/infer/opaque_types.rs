// ported from N:rustc_infer/src/infer/opaque_types/table.rs @8d1a764
use crate::consts::GenericArg;
use crate::program::DefId;
use crate::types::TyId;
use crate::fxhash::HashMap;
use super::snapshot::undo_log::{InferCtxtUndoLogs, UndoLog};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OpaqueTypeKey { pub def_id: DefId, pub args: Arc<[GenericArg]> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvisionalHiddenType { pub ty: TyId }

// Kept audit design: rustc_infer/src/infer/opaque_types/table.rs.
// The index map is equivalent to the nightly ordered map under LIFO undo.
#[derive(Default)]
pub struct OpaqueTypeStorage {
    opaque_types: Vec<(OpaqueTypeKey, ProvisionalHiddenType)>,
    indices: HashMap<OpaqueTypeKey, usize>,
    duplicate_entries: Vec<(OpaqueTypeKey, ProvisionalHiddenType)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct OpaqueTypeStorageEntries { opaque_types: usize, duplicate_entries: usize }

impl OpaqueTypeStorageEntries {
    pub fn needs_reevaluation(self, canonicalized: usize) -> bool { self.opaque_types != canonicalized }
}

impl OpaqueTypeStorage {
    pub(crate) fn remove(&mut self, key: OpaqueTypeKey, previous: Option<ProvisionalHiddenType>) {
        let index = self.indices[&key];
        if let Some(previous) = previous { self.opaque_types[index].1 = previous; }
        else {
            self.indices.remove(&key);
            let (popped_key, _) = self.opaque_types.swap_remove(index);
            assert_eq!(popped_key, key);
            if index < self.opaque_types.len() { self.indices.insert(self.opaque_types[index].0.clone(), index); }
        }
    }
    pub(crate) fn pop_duplicate_entry(&mut self) { self.duplicate_entries.pop().unwrap(); }
    pub fn is_empty(&self) -> bool { self.opaque_types.is_empty() && self.duplicate_entries.is_empty() }
    pub fn num_entries(&self) -> OpaqueTypeStorageEntries {
        OpaqueTypeStorageEntries { opaque_types: self.opaque_types.len(), duplicate_entries: self.duplicate_entries.len() }
    }
    pub fn opaque_types_added_since(&self, previous: OpaqueTypeStorageEntries) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)> {
        let mut result = self.opaque_types[previous.opaque_types..].to_vec();
        result.extend_from_slice(&self.duplicate_entries[previous.duplicate_entries..]);
        result
    }
    pub fn iter_lookup_table(&self) -> &[(OpaqueTypeKey, ProvisionalHiddenType)] { &self.opaque_types }
    pub fn iter_duplicate_entries(&self) -> &[(OpaqueTypeKey, ProvisionalHiddenType)] { &self.duplicate_entries }
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut InferCtxtUndoLogs) -> OpaqueTypeTable<'a> {
        OpaqueTypeTable { storage: self, undo_log }
    }
}

pub struct OpaqueTypeTable<'a> { storage: &'a mut OpaqueTypeStorage, undo_log: &'a mut InferCtxtUndoLogs }

impl std::ops::Deref for OpaqueTypeTable<'_> {
    type Target = OpaqueTypeStorage;
    fn deref(&self) -> &OpaqueTypeStorage { self.storage }
}

impl OpaqueTypeTable<'_> {
    pub fn register(&mut self, key: OpaqueTypeKey, hidden_type: ProvisionalHiddenType) -> Option<TyId> {
        if let Some(&index) = self.storage.indices.get(&key) {
            let previous = std::mem::replace(&mut self.storage.opaque_types[index].1, hidden_type);
            self.undo_log.push(UndoLog::OpaqueTypes(key, Some(previous)));
            return Some(previous.ty);
        }
        self.storage.indices.insert(key.clone(), self.storage.opaque_types.len());
        self.storage.opaque_types.push((key.clone(), hidden_type));
        self.undo_log.push(UndoLog::OpaqueTypes(key, None));
        None
    }
    pub fn add_duplicate(&mut self, key: OpaqueTypeKey, hidden_type: ProvisionalHiddenType) {
        self.storage.duplicate_entries.push((key, hidden_type));
        self.undo_log.push(UndoLog::DuplicateOpaqueType);
    }
}

impl super::InferCtxt<'_> {
    /// rustc_infer/src/infer/mod.rs:1132-1143: opaque hidden types may
    /// share a subtype-unification root without being equated yet.
    pub fn has_opaques_with_sub_unified_hidden_type(&self, var: u32) -> bool {
        let values = self.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
        for (_, hidden) in values {
            if let crate::types::TyKind::Infer(other) = self.tcx.tys.kind(self.shallow_resolve(hidden.ty)) {
                let mut inner = self.inner.borrow_mut();
                let mut table = inner.type_variables();
                if table.sub_unification_table_root_var(var) == table.sub_unification_table_root_var(*other) { return true; }
            }
        }
        false
    }
}
