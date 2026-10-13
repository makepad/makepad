// ported from N:rustc_infer/src/infer/type_variable.rs @8d1a764
use crate::types::{TyId, UniverseIndex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeVariableValue {
    Known { value: TyId },
    Unknown { universe: UniverseIndex },
}

impl TypeVariableValue {
    pub fn known(self) -> Option<TyId> {
        match self { Self::Known { value } => Some(value), Self::Unknown { .. } => None }
    }

    fn unify_values(self, other: Self) -> Self {
        match (self, other) {
            (Self::Known { .. }, Self::Known { .. }) => panic!("equating two known type variables"),
            (Self::Known { .. }, _) => self,
            (_, Self::Known { .. }) => other,
            (Self::Unknown { universe: a }, Self::Unknown { universe: b }) =>
                Self::Unknown { universe: a.min(b) },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EqRelation { parent: u32, value: TypeVariableValue }
#[derive(Clone, Copy)]
pub(crate) struct SubRelation { parent: u32, rank: u32 }

#[derive(Clone, Copy)]
pub(crate) enum UndoLog {
    AddVar,
    EqRelation(u32, EqRelation),
    SubRelation(u32, SubRelation),
}

#[derive(Clone, Default)]
pub struct TypeVariableStorage {
    eq_relations: Vec<EqRelation>,
    sub_unification_table: Vec<SubRelation>,
}

pub struct TypeVariableTable<'a> {
    storage: &'a mut TypeVariableStorage,
    undo_log: &'a mut super::snapshot::undo_log::InferCtxtUndoLogs,
}

impl TypeVariableStorage {
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut super::snapshot::undo_log::InferCtxtUndoLogs) -> TypeVariableTable<'a> {
        TypeVariableTable { storage: self, undo_log }
    }
    pub(crate) fn reverse(&mut self, undo: UndoLog) {
        match undo {
            UndoLog::AddVar => { self.eq_relations.pop().unwrap(); self.sub_unification_table.pop().unwrap(); }
            UndoLog::EqRelation(vid, old) => self.eq_relations[vid as usize] = old,
            UndoLog::SubRelation(vid, old) => self.sub_unification_table[vid as usize] = old,
        }
    }
}

impl TypeVariableTable<'_> {
    pub fn num_vars(&self) -> usize { self.storage.eq_relations.len() }

    pub fn new_var(&mut self, universe: UniverseIndex) -> u32 {
        let vid = self.num_vars() as u32;
        self.storage.eq_relations.push(EqRelation { parent: vid, value: TypeVariableValue::Unknown { universe } });
        self.storage.sub_unification_table.push(SubRelation { parent: vid, rank: 0 });
        self.log(UndoLog::AddVar);
        vid
    }

    pub fn root_var(&mut self, vid: u32) -> u32 {
        let parent = self.storage.eq_relations[vid as usize].parent;
        if vid == parent { return vid; }
        let root = self.root_var(parent);
        if root != parent {
            self.log(UndoLog::EqRelation(vid, self.storage.eq_relations[vid as usize]));
            self.storage.eq_relations[vid as usize].parent = root;
        }
        root
    }

    pub fn sub_unification_table_root_var(&mut self, vid: u32) -> u32 {
        let parent = self.storage.sub_unification_table[vid as usize].parent;
        if vid == parent { return vid; }
        let root = self.sub_unification_table_root_var(parent);
        if root != parent {
            self.log(UndoLog::SubRelation(vid, self.storage.sub_unification_table[vid as usize]));
            self.storage.sub_unification_table[vid as usize].parent = root;
        }
        root
    }

    pub fn probe(&mut self, vid: u32) -> TypeVariableValue {
        let root = self.root_var(vid);
        self.storage.eq_relations[root as usize].value
    }

    /// type_variable.rs:184,357: equality roots prefer the smaller TyVid;
    /// equality also joins the independent subtype component.
    pub fn equate(&mut self, a: u32, b: u32) {
        let a = self.root_var(a);
        let b = self.root_var(b);
        if a == b { return; }
        assert!(self.probe(a).known().is_none() && self.probe(b).known().is_none());
        let value = self.probe(a).unify_values(self.probe(b));
        let root = a.min(b);
        let child = a.max(b);
        self.log(UndoLog::EqRelation(root, self.storage.eq_relations[root as usize]));
        self.log(UndoLog::EqRelation(child, self.storage.eq_relations[child as usize]));
        self.storage.eq_relations[root as usize].value = value;
        self.storage.eq_relations[child as usize].parent = root;
        self.sub_unify(a, b);
    }

    pub fn sub_unify(&mut self, a: u32, b: u32) {
        assert!(self.probe(a).known().is_none() && self.probe(b).known().is_none());
        let mut a = self.sub_unification_table_root_var(a);
        let mut b = self.sub_unification_table_root_var(b);
        if a == b { return; }
        if self.storage.sub_unification_table[a as usize].rank < self.storage.sub_unification_table[b as usize].rank {
            std::mem::swap(&mut a, &mut b);
        }
        self.log(UndoLog::SubRelation(b, self.storage.sub_unification_table[b as usize]));
        self.storage.sub_unification_table[b as usize].parent = a;
        if self.storage.sub_unification_table[a as usize].rank == self.storage.sub_unification_table[b as usize].rank {
            self.log(UndoLog::SubRelation(a, self.storage.sub_unification_table[a as usize]));
            self.storage.sub_unification_table[a as usize].rank += 1;
        }
    }

    pub fn instantiate(&mut self, vid: u32, ty: TyId) {
        let root = self.root_var(vid);
        assert!(self.probe(root).known().is_none());
        self.log(UndoLog::EqRelation(root, self.storage.eq_relations[root as usize]));
        self.storage.eq_relations[root as usize].value = TypeVariableValue::Known { value: ty };
    }



    fn log(&mut self, undo: UndoLog) {
        self.undo_log.push(super::snapshot::undo_log::UndoLog::TypeVariables(undo));
    }
}
