// ported from N:rustc_infer/src/infer/unify_key.rs @8d1a764
// ported from N:rustc_type_ir/src/ty_kind.rs @8d1a764
use crate::consts::{Const};
use crate::program::Prim;
use crate::types::UniverseIndex;
use super::snapshot::undo_log::{InferCtxtUndoLogs, UndoLog};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstVariableValue { Known { value: Const }, Unknown { universe: UniverseIndex } }
impl ConstVariableValue {
    pub fn known(self) -> Option<Const> { match self { Self::Known { value } => Some(value), Self::Unknown { .. } => None } }
    fn unify_values(self, other: Self) -> Self {
        match (self, other) {
            (Self::Known { .. }, Self::Known { .. }) => panic!("equating two known const variables"),
            (known @ Self::Known { .. }, _) | (_, known @ Self::Known { .. }) => known,
            (Self::Unknown { universe: a }, Self::Unknown { universe: b }) => Self::Unknown { universe: a.min(b) },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntVarValue { Unknown, IntType(Prim), UintType(Prim) }
impl IntVarValue {
    fn unify_values(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, value) | (value, Self::Unknown) => value,
            _ => panic!("differing ints should have been resolved first"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatVarValue { Unknown, Known(Prim) }
impl FloatVarValue {
    fn unify_values(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, value) | (value, Self::Unknown) => value,
            _ => panic!("differing floats should have been resolved first"),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConstVar { parent: u32, rank: u32, value: ConstVariableValue }
#[derive(Default)]
pub struct ConstUnificationStorage { values: Vec<ConstVar> }
pub struct ConstUnificationTable<'a> { storage: &'a mut ConstUnificationStorage, undo_log: &'a mut InferCtxtUndoLogs }
impl ConstUnificationStorage {
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut InferCtxtUndoLogs) -> ConstUnificationTable<'a> {
        ConstUnificationTable { storage: self, undo_log }
    }
    pub(crate) fn reverse(&mut self, vid: u32, previous: Option<ConstVar>) {
        match previous { Some(previous) => self.values[vid as usize] = previous, None => { assert_eq!(vid as usize + 1, self.values.len()); self.values.pop().unwrap(); } }
    }
}
impl ConstUnificationTable<'_> {
    pub fn len(&self) -> usize { self.storage.values.len() }
    pub fn new_key(&mut self, value: ConstVariableValue) -> u32 {
        let vid = self.storage.values.len() as u32;
        self.storage.values.push(ConstVar { parent: vid, rank: 0, value });
        self.undo_log.push(UndoLog::ConstUnificationTable(vid, None));
        vid
    }
    pub fn find(&mut self, vid: u32) -> u32 {
        let parent = self.storage.values[vid as usize].parent;
        if parent == vid { return vid; }
        let root = self.find(parent);
        if root != parent {
            self.undo_log.push(UndoLog::ConstUnificationTable(vid, Some(self.storage.values[vid as usize])));
            self.storage.values[vid as usize].parent = root;
        }
        root
    }
    pub fn probe_value(&mut self, vid: u32) -> ConstVariableValue {
        let root = self.find(vid);
        self.storage.values[root as usize].value
    }
    pub fn union(&mut self, a: u32, b: u32) {
        let mut a = self.find(a);
        let mut b = self.find(b);
        if a == b { return; }
        let value = self.storage.values[a as usize].value.unify_values(self.storage.values[b as usize].value);
        if a > b { std::mem::swap(&mut a, &mut b); }
        self.undo_log.push(UndoLog::ConstUnificationTable(a, Some(self.storage.values[a as usize])));
        self.undo_log.push(UndoLog::ConstUnificationTable(b, Some(self.storage.values[b as usize])));
        self.storage.values[b as usize].parent = a;
        self.storage.values[a as usize].value = value;
        if self.storage.values[a as usize].rank == self.storage.values[b as usize].rank { self.storage.values[a as usize].rank += 1; }
    }
    pub fn union_value(&mut self, vid: u32, value: ConstVariableValue) {
        let root = self.find(vid);
        let value = self.storage.values[root as usize].value.unify_values(value);
        self.undo_log.push(UndoLog::ConstUnificationTable(root, Some(self.storage.values[root as usize])));
        self.storage.values[root as usize].value = value;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct IntVar { parent: u32, rank: u32, value: IntVarValue }
#[derive(Default)]
pub struct IntUnificationStorage { values: Vec<IntVar> }
pub struct IntUnificationTable<'a> { storage: &'a mut IntUnificationStorage, undo_log: &'a mut InferCtxtUndoLogs }
impl IntUnificationStorage {
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut InferCtxtUndoLogs) -> IntUnificationTable<'a> {
        IntUnificationTable { storage: self, undo_log }
    }
    pub(crate) fn reverse(&mut self, vid: u32, previous: Option<IntVar>) {
        match previous { Some(previous) => self.values[vid as usize] = previous, None => { assert_eq!(vid as usize + 1, self.values.len()); self.values.pop().unwrap(); } }
    }
}
impl IntUnificationTable<'_> {
    pub fn len(&self) -> usize { self.storage.values.len() }
    pub fn new_key(&mut self, value: IntVarValue) -> u32 {
        let vid = self.storage.values.len() as u32;
        self.storage.values.push(IntVar { parent: vid, rank: 0, value });
        self.undo_log.push(UndoLog::IntUnificationTable(vid, None));
        vid
    }
    pub fn find(&mut self, vid: u32) -> u32 {
        let parent = self.storage.values[vid as usize].parent;
        if parent == vid { return vid; }
        let root = self.find(parent);
        if root != parent {
            self.undo_log.push(UndoLog::IntUnificationTable(vid, Some(self.storage.values[vid as usize])));
            self.storage.values[vid as usize].parent = root;
        }
        root
    }
    pub fn probe_value(&mut self, vid: u32) -> IntVarValue {
        let root = self.find(vid);
        self.storage.values[root as usize].value
    }
    pub fn union(&mut self, a: u32, b: u32) {
        let mut a = self.find(a);
        let mut b = self.find(b);
        if a == b { return; }
        let value = self.storage.values[a as usize].value.unify_values(self.storage.values[b as usize].value);
        if self.storage.values[a as usize].rank < self.storage.values[b as usize].rank { std::mem::swap(&mut a, &mut b); }
        self.undo_log.push(UndoLog::IntUnificationTable(a, Some(self.storage.values[a as usize])));
        self.undo_log.push(UndoLog::IntUnificationTable(b, Some(self.storage.values[b as usize])));
        self.storage.values[b as usize].parent = a;
        self.storage.values[a as usize].value = value;
        if self.storage.values[a as usize].rank == self.storage.values[b as usize].rank { self.storage.values[a as usize].rank += 1; }
    }
    pub fn union_value(&mut self, vid: u32, value: IntVarValue) {
        let root = self.find(vid);
        let value = self.storage.values[root as usize].value.unify_values(value);
        self.undo_log.push(UndoLog::IntUnificationTable(root, Some(self.storage.values[root as usize])));
        self.storage.values[root as usize].value = value;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FloatVar { parent: u32, rank: u32, value: FloatVarValue }
#[derive(Default)]
pub struct FloatUnificationStorage { values: Vec<FloatVar> }
pub struct FloatUnificationTable<'a> { storage: &'a mut FloatUnificationStorage, undo_log: &'a mut InferCtxtUndoLogs }
impl FloatUnificationStorage {
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut InferCtxtUndoLogs) -> FloatUnificationTable<'a> {
        FloatUnificationTable { storage: self, undo_log }
    }
    pub(crate) fn reverse(&mut self, vid: u32, previous: Option<FloatVar>) {
        match previous { Some(previous) => self.values[vid as usize] = previous, None => { assert_eq!(vid as usize + 1, self.values.len()); self.values.pop().unwrap(); } }
    }
}
impl FloatUnificationTable<'_> {
    pub fn len(&self) -> usize { self.storage.values.len() }
    pub fn new_key(&mut self, value: FloatVarValue) -> u32 {
        let vid = self.storage.values.len() as u32;
        self.storage.values.push(FloatVar { parent: vid, rank: 0, value });
        self.undo_log.push(UndoLog::FloatUnificationTable(vid, None));
        vid
    }
    pub fn find(&mut self, vid: u32) -> u32 {
        let parent = self.storage.values[vid as usize].parent;
        if parent == vid { return vid; }
        let root = self.find(parent);
        if root != parent {
            self.undo_log.push(UndoLog::FloatUnificationTable(vid, Some(self.storage.values[vid as usize])));
            self.storage.values[vid as usize].parent = root;
        }
        root
    }
    pub fn probe_value(&mut self, vid: u32) -> FloatVarValue {
        let root = self.find(vid);
        self.storage.values[root as usize].value
    }
    pub fn union(&mut self, a: u32, b: u32) {
        let mut a = self.find(a);
        let mut b = self.find(b);
        if a == b { return; }
        let value = self.storage.values[a as usize].value.unify_values(self.storage.values[b as usize].value);
        if self.storage.values[a as usize].rank < self.storage.values[b as usize].rank { std::mem::swap(&mut a, &mut b); }
        self.undo_log.push(UndoLog::FloatUnificationTable(a, Some(self.storage.values[a as usize])));
        self.undo_log.push(UndoLog::FloatUnificationTable(b, Some(self.storage.values[b as usize])));
        self.storage.values[b as usize].parent = a;
        self.storage.values[a as usize].value = value;
        if self.storage.values[a as usize].rank == self.storage.values[b as usize].rank { self.storage.values[a as usize].rank += 1; }
    }
    pub fn union_value(&mut self, vid: u32, value: FloatVarValue) {
        let root = self.find(vid);
        let value = self.storage.values[root as usize].value.unify_values(value);
        self.undo_log.push(UndoLog::FloatUnificationTable(root, Some(self.storage.values[root as usize])));
        self.storage.values[root as usize].value = value;
    }
}
