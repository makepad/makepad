//! rustc_mir_dataflow/src/move_paths/mod.rs:171,242,293,391.
//! Paths, moves and initializations are separate flat arenas. All indices are usize.
use super::*;
use crate::program::Program;
use crate::tcx::Tcx;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};

mod builder;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct MovePath {
    pub next_sibling: Option<usize>,
    pub first_child: Option<usize>,
    pub parent: Option<usize>,
    pub place: Place,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveOut { pub path: usize, pub source: Location }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitKind { Deep, Shallow, NonPanicPathOnly }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitLocation { Argument(Local), Statement(Location) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Init { pub path: usize, pub location: InitLocation, pub kind: InitKind }

/// Both location maps contain arena indices; block offsets include the terminator.
#[derive(Debug)]
pub struct LocationMap { offsets: Vec<usize>, entries: Vec<Vec<usize>> }
impl LocationMap {
    fn new(body: &Body) -> Self {
        let mut offsets = vec![0];
        let mut entries = Vec::new();
        for block in &body.basic_blocks {
            for _ in 0..=block.statements.len() { entries.push(Vec::new()); }
            offsets.push(entries.len());
        }
        Self { offsets, entries }
    }
    fn index_of(&self, loc: Location) -> usize {
        let block = loc.block.0 as usize;
        let index = self.offsets[block] + loc.statement_index;
        assert!(index < self.offsets[block + 1]);
        index
    }
}
impl Index<Location> for LocationMap {
    type Output = Vec<usize>;
    fn index(&self, loc: Location) -> &Self::Output { &self.entries[self.index_of(loc)] }
}
impl IndexMut<Location> for LocationMap {
    fn index_mut(&mut self, loc: Location) -> &mut Self::Output {
        let index = self.index_of(loc);
        &mut self.entries[index]
    }
}

#[derive(Debug)]
pub struct MoveData {
    pub move_paths: Vec<MovePath>,
    pub moves: Vec<MoveOut>,
    pub inits: Vec<Init>,
    pub loc_map: LocationMap,
    pub init_loc_map: LocationMap,
    pub path_map: Vec<Vec<usize>>,
    pub init_path_map: Vec<Vec<usize>>,
    pub rev_lookup: MovePathLookup,
}
impl MoveData {
    /// The filter is supplied by the consumer, as in rustc builder.rs:333.
    /// This layer never queries Copy or needs_drop and never diagnoses illegal moves.
    pub fn gather_moves(body: &Body, prog: &Program, tcx: &Tcx, env: crate::tcx::TypingEnv, filter: &dyn Fn(TyId) -> bool) -> Self {
        builder::gather_moves(body, prog, tcx, env, filter)
    }
    pub fn base_local(&self, mut path: usize) -> Local {
        while let Some(parent) = self.move_paths[path].parent { path = parent; }
        self.move_paths[path].place.local
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupResult { Exact(usize), Parent(Option<usize>) }
#[derive(Debug)]
pub struct MovePathLookup {
    locals: Vec<Option<usize>>,
    projections: HashMap<(usize, MoveSubPath), usize>,
    /// rustc_mir_dataflow/src/un_derefer.rs:6. Chains undo DerefSeparator;
    /// saved Places retain the type-bearing prefixes of each temporary.
    deref_chains: Vec<Vec<Place>>,
}
impl MovePathLookup {
    pub fn find_local(&self, local: Local) -> Option<usize> { self.locals[local.0 as usize] }
    pub fn find(&self, place: &Place) -> LookupResult {
        let Some(mut path) = self.find_local(place.local) else { return LookupResult::Parent(None); };
        for (_, _, elem) in self.iter_projections(place) {
            match MoveSubPath::of(elem) {
                SubPath::One(key) => match self.projections.get(&(path, key)) {
                    Some(&child) => path = child,
                    None => return LookupResult::Parent(Some(path)),
                },
                SubPath::Skip => (),
                SubPath::Stop | SubPath::Subslice { .. } => return LookupResult::Parent(Some(path)),
            }
        }
        LookupResult::Exact(path)
    }
    fn iter_projections<'a>(&'a self, place: &'a Place) -> ProjectionIter<'a> {
        let chain = &self.deref_chains[place.local.0 as usize];
        if place.projection.is_empty() { assert!(chain.is_empty(), "bare deref temporary"); }
        ProjectionIter { chain, place, chain_index: 0, projection_index: 0 }
    }
}

/// Each prefix is relative to the place that supplied it, including DerefTemps.
struct ProjectionIter<'a> {
    chain: &'a [Place], place: &'a Place, chain_index: usize, projection_index: usize,
}
impl<'a> Iterator for ProjectionIter<'a> {
    type Item = (&'a Place, usize, &'a PlaceElem);
    fn next(&mut self) -> Option<Self::Item> {
        let place = if self.chain_index < self.chain.len() { &self.chain[self.chain_index] }
            else if self.chain_index == self.chain.len() { self.place } else { return None; };
        let elem = place.projection.get(self.projection_index)?;
        let prefix_len = self.projection_index;
        self.projection_index += 1;
        if self.projection_index == place.projection.len() { self.chain_index += 1; self.projection_index = 0; }
        Some((place, prefix_len, elem))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MoveSubPath { Deref, Field(FieldIdx), ConstantIndex(u64), Downcast(VariantIdx) }
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubPath { One(MoveSubPath), Skip, Stop, Subslice { from: u64, to: u64 } }
impl MoveSubPath {
    fn of(elem: &PlaceElem) -> SubPath {
        SubPath::One(match elem {
            PlaceElem::Deref => Self::Deref,
            PlaceElem::Field(field, _) => Self::Field(*field),
            PlaceElem::Downcast(_, variant) => Self::Downcast(*variant),
            PlaceElem::ConstantIndex { offset, from_end: false, .. } => Self::ConstantIndex(*offset),
            PlaceElem::OpaqueCast(_) | PlaceElem::Subtype(_) => return SubPath::Skip,
            PlaceElem::Index(_) | PlaceElem::ConstantIndex { from_end: true, .. }
            | PlaceElem::Subslice { from_end: true, .. } => return SubPath::Stop,
            PlaceElem::Subslice { from, to, from_end: false } => return SubPath::Subslice { from: *from, to: *to },
        })
    }
}
