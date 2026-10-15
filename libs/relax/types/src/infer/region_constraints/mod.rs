// ported from N:rustc_infer/src/infer/region_constraints/mod.rs @8d1a764
// ported from N:rustc_infer/src/infer/unify_key.rs @8d1a764
use crate::types::{RegionId, RegionKind, Types, UniverseIndex};

mod leak_check;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintKind { VarSubVar, RegSubVar, VarSubReg, RegSubReg, VarEqVar, VarEqReg, RegEqReg }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub sub: RegionId,
    pub sup: RegionId,
    pub visible_for_leak_check: bool,
}

#[derive(Clone, Copy, Debug)]
enum RegionVariableValue {
    Known { value: RegionId },
    Unknown { universe: UniverseIndex },
}

impl RegionVariableValue {
    fn unify_values(self, other: Self, tys: &Types) -> Option<Self> {
        match (self, other) {
            (Self::Known { .. }, Self::Known { .. }) => None,
            (Self::Known { value }, Self::Unknown { universe }) |
            (Self::Unknown { universe }, Self::Known { value }) => {
                let value_universe = match tys.region_kind(value) {
                    RegionKind::RePlaceholder { universe, .. } => universe,
                    RegionKind::ReVar(_) | RegionKind::ReBound(..) => panic!("not a universal region"),
                    _ => UniverseIndex::ROOT,
                };
                if universe.can_name(value_universe) { Some(Self::Known { value }) } else { None }
            }
            (Self::Unknown { universe: a }, Self::Unknown { universe: b }) =>
                Some(Self::Unknown { universe: a.min(b) }),
        }
    }
}

/// mod.rs RegionVariableOrigin: only the NLL origins are observed (by borrowck's
/// region definitions); every other origin is diagnostic-only (DEBLOAT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionVariableOrigin { Misc, Nll(crate::borrowck::region_infer::NllRegionVariableOrigin) }

#[derive(Clone, Copy)]
pub(crate) struct RegionVariableInfo { pub universe: UniverseIndex, pub origin: RegionVariableOrigin, parent: u32, rank: u32, value: RegionVariableValue }

#[derive(Clone, Default)]
pub struct RegionConstraintStorage {
    pub(crate) var_infos: Vec<RegionVariableInfo>,
    pub constraints: Vec<Constraint>,
    pub verifys: Vec<crate::infer::outlives::verify::Verify>,
    combinations: std::collections::HashMap<(bool, RegionId, RegionId), RegionId>,
    any_unifications: bool,
}

/// RegionConstraintData: what take_and_reset_data hands to NLL.
pub struct RegionConstraintData { pub constraints: Vec<Constraint>, pub verifys: Vec<crate::infer::outlives::verify::Verify> }

pub(crate) enum UndoLog { AddCombination(bool, RegionId, RegionId), AddVar(usize), AddConstraint(usize), AddVerify(usize), Unification(u32, RegionVariableInfo) }

pub struct RegionConstraintCollector<'a> {
    storage: &'a mut RegionConstraintStorage,
    undo_log: &'a mut super::snapshot::undo_log::InferCtxtUndoLogs,
}

pub struct RegionSnapshot { any_unifications: bool }

impl RegionConstraintStorage {
    pub(crate) fn with_log<'a>(&'a mut self, undo_log: &'a mut super::snapshot::undo_log::InferCtxtUndoLogs) -> RegionConstraintCollector<'a> {
        RegionConstraintCollector { storage: self, undo_log }
    }
    pub(crate) fn reverse(&mut self, undo: UndoLog) {
        match undo {
            UndoLog::AddCombination(lub, a, b) => { self.combinations.remove(&(lub, a, b)).unwrap(); }
            UndoLog::AddVar(index) => { assert_eq!(self.var_infos.len(), index + 1); self.var_infos.pop().unwrap(); }
            UndoLog::AddConstraint(index) => { assert_eq!(self.constraints.len(), index + 1); self.constraints.pop().unwrap(); }
            UndoLog::AddVerify(index) => { assert_eq!(self.verifys.len(), index + 1); self.verifys.pop().unwrap(); }
            UndoLog::Unification(vid, old) => self.var_infos[vid as usize] = old,
        }
    }
}

impl RegionConstraintCollector<'_> {
    /// mod.rs:635-736: cache each ordered pair and undo cache insertions with
    /// their fresh region. The result can name both input universes.
    pub fn lub_regions(&mut self, tys: &Types, a: RegionId, b: RegionId) -> RegionId {
        if tys.region_kind(a) == RegionKind::ReStatic || tys.region_kind(b) == RegionKind::ReStatic || a == b { a }
        else { self.combine_vars(tys, true, a, b) }
    }
    pub fn glb_regions(&mut self, tys: &Types, a: RegionId, b: RegionId) -> RegionId {
        if tys.region_kind(a) == RegionKind::ReStatic { b }
        else if tys.region_kind(b) == RegionKind::ReStatic || a == b { a }
        else { self.combine_vars(tys, false, a, b) }
    }
    fn combine_vars(&mut self, tys: &Types, lub: bool, a: RegionId, b: RegionId) -> RegionId {
        if let Some(&region) = self.storage.combinations.get(&(lub, a, b)) { return region; }
        let universe = self.universe(tys, a).max(self.universe(tys, b));
        let region = self.new_region_var(tys, universe);
        self.storage.combinations.insert((lub, a, b), region);
        self.log(UndoLog::AddCombination(lub, a, b));
        for old in [a, b] {
            if lub { self.make_subregion(tys, old, region, true); }
            else { self.make_subregion(tys, region, old, true); }
        }
        region
    }
    pub fn num_region_vars(&self) -> usize { self.storage.var_infos.len() }

    pub fn new_region_var(&mut self, tys: &Types, universe: UniverseIndex) -> RegionId {
        self.new_region_var_with_origin(tys, universe, RegionVariableOrigin::Misc)
    }

    pub fn new_region_var_with_origin(&mut self, tys: &Types, universe: UniverseIndex, origin: RegionVariableOrigin) -> RegionId {
        let vid = self.storage.var_infos.len() as u32;
        self.storage.var_infos.push(RegionVariableInfo { universe, origin, parent: vid, rank: 0, value: RegionVariableValue::Unknown { universe } });
        self.log(UndoLog::AddVar(vid as usize));
        tys.intern_region(RegionKind::ReVar(vid))
    }

    pub fn root_var(&mut self, vid: u32) -> u32 {
        let parent = self.storage.var_infos[vid as usize].parent;
        if parent == vid { return vid; }
        let root = self.root_var(parent);
        if root != parent {
            self.log(UndoLog::Unification(vid, self.storage.var_infos[vid as usize]));
            self.storage.var_infos[vid as usize].parent = root;
        }
        root
    }

    pub fn shallow_resolve_region_var(&mut self, tys: &Types, vid: u32) -> RegionId {
        let root = self.root_var(vid);
        match self.storage.var_infos[root as usize].value {
            RegionVariableValue::Known { value } => value,
            RegionVariableValue::Unknown { .. } => tys.intern_region(RegionKind::ReVar(root)),
        }
    }

    pub fn universe(&mut self, tys: &Types, region: RegionId) -> UniverseIndex {
        match tys.region_kind(region) {
            RegionKind::RePlaceholder { universe, .. } => universe,
            RegionKind::ReVar(vid) => {
                let root = self.root_var(vid);
                match self.storage.var_infos[root as usize].value {
                    RegionVariableValue::Known { value } => self.universe(tys, value),
                    RegionVariableValue::Unknown { universe } => universe,
                }
            }
            RegionKind::ReBound(..) => panic!("universe of bound region"),
            _ => UniverseIndex::ROOT,
        }
    }

    pub fn make_subregion(&mut self, tys: &Types, sub: RegionId, sup: RegionId, visible: bool) {
        use RegionKind::*;
        let kind = match (tys.region_kind(sub), tys.region_kind(sup)) {
            (ReBound(..), _) | (_, ReBound(..)) => panic!("cannot relate bound region"),
            (_, ReStatic) => return,
            (ReVar(a), ReVar(b)) => { if a == b { return; } ConstraintKind::VarSubVar }
            (_, ReVar(_)) => ConstraintKind::RegSubVar,
            (ReVar(_), _) => ConstraintKind::VarSubReg,
            _ => { if sub == sup { return; } ConstraintKind::RegSubReg }
        };
        self.log(UndoLog::AddConstraint(self.storage.constraints.len()));
        self.storage.constraints.push(Constraint { kind, sub, sup, visible_for_leak_check: visible });
    }

    pub fn make_eqregion(&mut self, tys: &Types, a: RegionId, b: RegionId, visible: bool) {
        use RegionKind::*;
        if a == b { return; }
        let (kind, sub, sup) = match (tys.region_kind(a), tys.region_kind(b)) {
            (ReBound(..), _) | (_, ReBound(..)) => panic!("cannot relate bound region"),
            (ReVar(av), ReVar(bv)) => {
                self.unify_var_var(tys, av, bv);
                (ConstraintKind::VarEqVar, a, b)
            }
            (ReVar(vid), _) => {
                self.unify_var_value(tys, vid, b);
                if tys.region_kind(b) == ReStatic { (ConstraintKind::RegSubVar, b, a) }
                else { (ConstraintKind::VarEqReg, a, b) }
            }
            (_, ReVar(vid)) => {
                self.unify_var_value(tys, vid, a);
                if tys.region_kind(a) == ReStatic { (ConstraintKind::RegSubVar, a, b) }
                else { (ConstraintKind::VarEqReg, b, a) }
            }
            (ReStatic, _) => (ConstraintKind::RegSubReg, a, b),
            (_, ReStatic) => (ConstraintKind::RegSubReg, b, a),
            _ => (ConstraintKind::RegEqReg, a, b),
        };
        self.log(UndoLog::AddConstraint(self.storage.constraints.len()));
        self.storage.constraints.push(Constraint { kind, sub, sup, visible_for_leak_check: visible });
    }

    fn unify_var_value(&mut self, tys: &Types, vid: u32, value: RegionId) {
        let root = self.root_var(vid);
        if let Some(value) = self.storage.var_infos[root as usize].value.unify_values(RegionVariableValue::Known { value }, tys) {
            self.log(UndoLog::Unification(root, self.storage.var_infos[root as usize]));
            self.storage.any_unifications = true;
            self.storage.var_infos[root as usize].value = value;
        }
    }

    fn unify_var_var(&mut self, tys: &Types, a: u32, b: u32) {
        let mut a = self.root_var(a);
        let mut b = self.root_var(b);
        if a == b { return; }
        let Some(value) = self.storage.var_infos[a as usize].value.unify_values(self.storage.var_infos[b as usize].value, tys) else { return; };
        if self.storage.var_infos[a as usize].rank < self.storage.var_infos[b as usize].rank { std::mem::swap(&mut a, &mut b); }
        self.log(UndoLog::Unification(a, self.storage.var_infos[a as usize]));
        self.log(UndoLog::Unification(b, self.storage.var_infos[b as usize]));
        self.storage.any_unifications = true;
        self.storage.var_infos[b as usize].parent = a;
        self.storage.var_infos[a as usize].value = value;
        if self.storage.var_infos[a as usize].rank == self.storage.var_infos[b as usize].rank { self.storage.var_infos[a as usize].rank += 1; }
    }

    pub(crate) fn start_snapshot(&self) -> RegionSnapshot { RegionSnapshot { any_unifications: self.storage.any_unifications } }
    pub(crate) fn rollback_to(&mut self, snapshot: RegionSnapshot) { self.storage.any_unifications = snapshot.any_unifications; }
    pub fn constraints(&self) -> &[Constraint] { &self.storage.constraints }
    pub fn var_origin(&self, vid: u32) -> RegionVariableOrigin { self.storage.var_infos[vid as usize].origin }

    pub fn verify_generic_bound(&mut self, verify: crate::infer::outlives::verify::Verify) {
        if matches!(&verify.bound, crate::infer::outlives::verify::VerifyBound::AllBounds(bs) if bs.is_empty()) { return; }
        self.log(UndoLog::AddVerify(self.storage.verifys.len()));
        self.storage.verifys.push(verify);
    }

    /// mod.rs:384-421: hands the constraints to NLL and undoes every unification,
    /// whose effect the taken data records as two-way edges.
    pub fn take_and_reset_data(&mut self) -> RegionConstraintData {
        assert!(!self.undo_log.in_snapshot());
        self.storage.combinations.clear();
        let constraints = std::mem::take(&mut self.storage.constraints);
        let verifys = std::mem::take(&mut self.storage.verifys);
        if self.storage.any_unifications {
            self.storage.any_unifications = false;
            for (vid, info) in self.storage.var_infos.iter_mut().enumerate() {
                info.parent = vid as u32;
                info.rank = 0;
                info.value = RegionVariableValue::Unknown { universe: info.universe };
            }
        }
        RegionConstraintData { constraints, verifys }
    }
    fn log(&mut self, undo: UndoLog) { self.undo_log.push(super::snapshot::undo_log::UndoLog::RegionConstraintCollector(undo)); }
}
