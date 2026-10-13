// ported from N:rustc_infer/src/infer/lexical_region_resolve/mod.rs @8d1a764
use super::free_regions::RegionRelations;
use super::region_constraints::{Constraint, ConstraintKind, RegionVariableInfo};
use super::outlives::verify::{Verify, VerifyBound};
use crate::types::{RegionId, RegionKind, TyId, Types, UniverseIndex};
use crate::types::fold::TypeFolder;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionResolutionError {
    ConcreteFailure(RegionId, RegionId),
    GenericBoundFailure(TyId, RegionId),
    SubSupConflict(u32, RegionId, RegionId),
    UpperBoundUniverseConflict(u32, UniverseIndex, RegionId),
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum VarValue { Empty(UniverseIndex), Value(RegionId), ErrorValue }
#[derive(Clone)]
pub(crate) struct LexicalRegionResolutions { pub values: Vec<VarValue> }
impl LexicalRegionResolutions {
    pub fn resolve_region(&self, tys: &Types, r: RegionId) -> RegionId {
        if let RegionKind::ReVar(vid) = tys.region_kind(r) {
            match self.values[vid as usize] { VarValue::Empty(_) => r, VarValue::Value(r) => r, VarValue::ErrorValue => tys.intern_region(RegionKind::ReStatic) }
        } else { r }
    }
    fn value(&self, tys: &Types, r: RegionId) -> VarValue {
        if let RegionKind::ReVar(vid) = tys.region_kind(r) { self.values[vid as usize] } else { VarValue::Value(r) }
    }
}
impl TypeFolder for LexicalRegionResolutions {
    fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId { self.resolve_region(tys, r) }
}

pub(crate) fn resolve(region_rels: &RegionRelations<'_>, diagnostics: &super::diagnostics::DiagCtxt, var_infos: Vec<RegionVariableInfo>, mut constraints: Vec<Constraint>, verifys: Vec<Verify>) -> (LexicalRegionResolutions, Vec<RegionResolutionError>) {
    assert!(constraints.iter().all(|c| !matches!(c.kind, ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg)), "equality must be decomposed before lexical resolution");
    let mut seen = HashSet::new();
    constraints.retain_mut(|c| { c.visible_for_leak_check = false; seen.insert(*c) });
    let resolver = LexicalResolver { region_rels, diagnostics, var_infos, constraints, verifys };
    let mut errors = Vec::new();
    let mut values = LexicalRegionResolutions { values: Vec::new() };
    for info in &resolver.var_infos { values.values.push(VarValue::Empty(info.universe)); }
    resolver.expansion(&mut values);
    resolver.collect_errors(&mut values, &mut errors);
    resolver.collect_var_errors(&values, &mut errors);
    (values, errors)
}
struct LexicalResolver<'a> {
    region_rels: &'a RegionRelations<'a>,
    diagnostics: &'a super::diagnostics::DiagCtxt,
    var_infos: Vec<RegionVariableInfo>,
    constraints: Vec<Constraint>,
    verifys: Vec<Verify>,
}
impl LexicalResolver<'_> {
    fn vid(&self, r: RegionId) -> usize { let RegionKind::ReVar(vid) = self.region_rels.tys.region_kind(r) else { panic!("expected region variable") }; vid as usize }
    fn expansion(&self, values: &mut LexicalRegionResolutions) {
        let tys = self.region_rels.tys;
        let static_ = tys.intern_region(RegionKind::ReStatic);
        let mut constraints = vec![Vec::new(); values.values.len()];
        let mut changes = Vec::new();
        for c in &self.constraints {
            match c.kind {
                ConstraintKind::RegSubVar => {
                    let b = self.vid(c.sup);
                    if self.expand_node(c.sub, b, &mut values.values[b]) { changes.push(b); }
                }
                ConstraintKind::VarSubVar => {
                    let a = self.vid(c.sub); let b = self.vid(c.sup);
                    let changed = match values.values[a] {
                        VarValue::ErrorValue => continue,
                        VarValue::Empty(au) => match values.values[b] {
                            VarValue::Empty(bu) => { values.values[b] = VarValue::Empty(au.min(bu)); true }
                            VarValue::Value(r) => match tys.region_kind(r) {
                                RegionKind::RePlaceholder { universe, .. } if !au.can_name(universe) => { values.values[b] = VarValue::Value(static_); true }
                                _ => false,
                            },
                            VarValue::ErrorValue => false,
                        },
                        VarValue::Value(r) => self.expand_node(r, b, &mut values.values[b]),
                    };
                    if changed { changes.push(b); }
                    if !matches!(values.values[b], VarValue::ErrorValue) && !matches!(values.values[b], VarValue::Value(r) if r == static_) {
                        constraints[a].push((a, b)); constraints[b].push((a, b));
                    }
                }
                ConstraintKind::VarSubReg | ConstraintKind::RegSubReg => {},
                _ => panic!("equality must be decomposed before region resolution"),
            }
        }
        while let Some(vid) = changes.pop() {
            constraints[vid].retain(|&(a, b)| {
                let VarValue::Value(r) = values.values[a] else { return false; };
                if self.expand_node(r, b, &mut values.values[b]) { changes.push(b); }
                !matches!(values.values[b], VarValue::ErrorValue) && !matches!(values.values[b], VarValue::Value(r) if r == static_)
            });
        }
    }
    fn expand_node(&self, a: RegionId, b: usize, value: &mut VarValue) -> bool {
        let tys = self.region_rels.tys;
        let (mut lub, universe) = match *value {
            VarValue::Empty(u) => (a, u),
            VarValue::Value(r) => { let lub = self.region_rels.lub_concrete_regions(a, r); if lub == r { return false; } (lub, self.var_infos[b].universe) }
            VarValue::ErrorValue => return false,
        };
        if let RegionKind::RePlaceholder { universe: u, .. } = tys.region_kind(lub) {
            if !universe.can_name(u) { lub = tys.intern_region(RegionKind::ReStatic); }
        }
        *value = VarValue::Value(lub); true
    }
    fn sub_region_values(&self, a: VarValue, b: VarValue) -> bool {
        match (a, b) {
            (VarValue::ErrorValue, _) | (_, VarValue::ErrorValue) => true,
            (VarValue::Empty(a), VarValue::Empty(b)) => a.min(b) == b,
            (VarValue::Value(_), VarValue::Empty(_)) => false,
            (VarValue::Empty(a), VarValue::Value(b)) => match self.region_rels.tys.region_kind(b) {
                RegionKind::RePlaceholder { universe, .. } => a.can_name(universe),
                RegionKind::ReError => false,
                RegionKind::ReBound(..) | RegionKind::ReErased | RegionKind::ReVar(_) => panic!("non-concrete region"),
                _ => true,
            },
            (VarValue::Value(a), VarValue::Value(b)) => self.region_rels.sub_concrete_regions(a, b),
        }
    }
    fn collect_errors(&self, values: &mut LexicalRegionResolutions, errors: &mut Vec<RegionResolutionError>) {
        for c in &self.constraints {
            match c.kind {
                ConstraintKind::RegSubReg if !self.region_rels.sub_concrete_regions(c.sub, c.sup) => errors.push(RegionResolutionError::ConcreteFailure(c.sub, c.sup)),
                ConstraintKind::VarSubReg => {
                    let vid = self.vid(c.sub);
                    if let VarValue::Value(r) = values.values[vid] {
                        if !self.region_rels.sub_concrete_regions(r, c.sup) { values.values[vid] = VarValue::ErrorValue; }
                    }
                }
                _ => {},
            }
        }
        for verify in &self.verifys {
            let sub = values.resolve_region(self.region_rels.tys, verify.region);
            let ty = values.fold_ty(self.region_rels.tys, verify.ty, 0);
            if !self.bound_is_met(&verify.bound, values, ty, sub) { errors.push(RegionResolutionError::GenericBoundFailure(verify.ty, sub)); }
        }
    }
    fn bound_is_met(&self, bound: &VerifyBound, values: &LexicalRegionResolutions, ty: TyId, min: RegionId) -> bool {
        let tys = self.region_rels.tys;
        if tys.region_kind(min) == RegionKind::ReError { return true; }
        match bound {
            VerifyBound::OutlivedBy(r) => self.sub_region_values(values.value(tys, min), values.value(tys, *r)),
            VerifyBound::IsEmpty => matches!(values.value(tys, min), VarValue::Empty(_)),
            VerifyBound::AnyBound(bs) => { for b in bs { if self.bound_is_met(b, values, ty, min) { return true; } } false }
            VerifyBound::AllBounds(bs) => { for b in bs { if !self.bound_is_met(b, values, ty, min) { return false; } } true }
            VerifyBound::IfEq(predicate) => {
                let predicate = crate::next_solver::fold::fold_predicate(tys, predicate, &mut values.clone());
                match super::outlives::test_type_match::extract_verify_if_eq(self.region_rels.tcx, self.region_rels.prog, &predicate, ty) {
                    Some(r) => self.bound_is_met(&VerifyBound::OutlivedBy(r), values, ty, min), None => false,
                }
            }
        }
    }
    fn collect_bounding_regions(&self, start: usize, incoming: bool, duplicates: &mut [Option<usize>]) -> (Vec<RegionId>, Vec<usize>, bool) {
        let mut visited = HashSet::new(); let mut stack = vec![start]; let mut regions = Vec::new(); let mut vars = Vec::new(); let mut duplicate = false;
        visited.insert(start);
        while let Some(vid) = stack.pop() {
            vars.push(vid);
            if let Some(previous) = duplicates[vid] { duplicate |= previous != start; } else { duplicates[vid] = Some(start); }
            for c in &self.constraints {
                match c.kind {
                    ConstraintKind::VarSubVar => {
                        let (a, b) = if incoming { (self.vid(c.sup), self.vid(c.sub)) } else { (self.vid(c.sub), self.vid(c.sup)) };
                        if a == vid && visited.insert(b) { stack.push(b); }
                    }
                    ConstraintKind::RegSubVar if incoming && self.vid(c.sup) == vid => regions.push(c.sub),
                    ConstraintKind::VarSubReg if !incoming && self.vid(c.sub) == vid => regions.push(c.sup),
                    _ => {},
                }
            }
        }
        (regions, vars, duplicate)
    }
    fn collect_var_errors(&self, values: &LexicalRegionResolutions, errors: &mut Vec<RegionResolutionError>) {
        let mut duplicates = vec![None; values.values.len()];
        for vid in 0..values.values.len() {
            if !matches!(values.values[vid], VarValue::ErrorValue) { continue; }
            let (mut lower, vars, ld) = self.collect_bounding_regions(vid, true, &mut duplicates);
            let (mut upper, _, ud) = self.collect_bounding_regions(vid, false, &mut duplicates);
            if ld || ud { continue; }
            // lexical_region_resolve/mod.rs:715-723: stable diagnostic preference.
            let key = |r: &RegionId| match self.region_rels.tys.region_kind(*r) { RegionKind::ReEarlyParam(_) => 0, RegionKind::ReLateParam(..) => 1, _ => 2 };
            lower.sort_by_key(key); upper.sort_by_key(key);
            let mut found = false;
            'bounds: for &a in &lower {
                let effective = match self.region_rels.tys.region_kind(a) {
                    RegionKind::RePlaceholder { universe, .. } if !self.var_infos[vid].universe.can_name(universe) => self.region_rels.tys.intern_region(RegionKind::ReStatic), _ => a,
                };
                for &b in &upper { if !self.region_rels.sub_concrete_regions(effective, b) { errors.push(RegionResolutionError::SubSupConflict(vid as u32, a, b)); found = true; break 'bounds; } }
            }
            if found { continue; }
            let mut min = self.var_infos[vid].universe;
            for v in vars { min = min.min(self.var_infos[v].universe); }
            for b in upper {
                if let RegionKind::RePlaceholder { universe, .. } = self.region_rels.tys.region_kind(b) {
                    if !min.can_name(universe) { errors.push(RegionResolutionError::UpperBoundUniverseConflict(vid as u32, min, b)); found = true; break; }
                }
            }
            assert!(found || self.diagnostics.has_errors(), "error region without conflicting bounds");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round31_lexical_error_variable_requires_root_error() {
        let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
        let tcx = crate::tcx::Tcx::new();
        let infcx = crate::infer::InferCtxt::new(&tcx, crate::tcx::TypingMode::PostAnalysis, Default::default());
        infcx.next_region_var();
        let storage = infcx.inner.borrow_mut().region_constraint_storage.take().unwrap();
        let free_regions = crate::infer::free_regions::FreeRegionMap::default();
        let relations = RegionRelations { prog: &prog, tcx: &tcx, tys: &tcx.tys, free_regions: &free_regions };
        for state in 0..3 {
            let diagnostics = crate::infer::diagnostics::DiagCtxt::default();
            if state == 1 { infcx.set_tainted_by_errors(); }
            if state == 2 { diagnostics.report(Default::default(), "earlier root error".into()); }
            let resolver = LexicalResolver { region_rels: &relations, diagnostics: &diagnostics,
                var_infos: storage.var_infos.clone(), constraints: Vec::new(), verifys: Vec::new() };
            let values = LexicalRegionResolutions { values: vec![VarValue::ErrorValue] };
            // A locally accumulated region error does not count as a reported root error.
            let mut errors = vec![RegionResolutionError::ConcreteFailure(RegionId::ERASED, RegionId::ERASED)];
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolver.collect_var_errors(&values, &mut errors)));
            assert_eq!(result.is_ok(), state == 2);
        }
    }
}
