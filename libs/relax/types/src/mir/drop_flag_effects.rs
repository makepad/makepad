//! rustc_mir_dataflow/src/drop_flag_effects.rs:28,50,65,90,106,137,176.
use super::*;
use super::move_paths::{InitKind, LookupResult, MoveData};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropFlagState { Present, Absent }
impl DropFlagState {
    pub fn value(self) -> bool { self == Self::Present }
}

pub fn move_path_children_matching(data: &MoveData, path: usize, cond: &mut dyn FnMut(&PlaceElem) -> bool) -> Option<usize> {
    let mut child = data.move_paths[path].first_child;
    while let Some(index) = child {
        let path = &data.move_paths[index];
        if let Some(elem) = path.place.projection.last() { if cond(elem) { return Some(index); } }
        child = path.next_sibling;
    }
    None
}

pub fn on_lookup_result_bits(data: &MoveData, lookup: LookupResult, callback: &mut dyn FnMut(usize)) {
    if let LookupResult::Exact(path) = lookup { on_all_children_bits(data, path, callback); }
}

/// Preorder, including the root; Parent lookups never affect a tracked ancestor.
/// Parent/sibling links avoid recursive stack growth for deeply projected places.
pub fn on_all_children_bits(data: &MoveData, root: usize, callback: &mut dyn FnMut(usize)) {
    let mut path = root;
    loop {
        callback(path);
        if let Some(child) = data.move_paths[path].first_child { path = child; continue; }
        loop {
            if path == root { return; }
            if let Some(sibling) = data.move_paths[path].next_sibling { path = sibling; break; }
            path = data.move_paths[path].parent.unwrap();
        }
    }
}

pub fn drop_flag_effects_for_function_entry(body: &Body, data: &MoveData, callback: &mut dyn FnMut(usize, DropFlagState)) {
    for arg in 1..=body.arg_count {
        on_lookup_result_bits(data, data.rev_lookup.find(&Place::local(Local(arg))), &mut |path| callback(path, DropFlagState::Present));
    }
}

pub fn drop_flag_effects_for_location(body: &Body, data: &MoveData, loc: Location, callback: &mut dyn FnMut(usize, DropFlagState)) {
    for &index in &data.loc_map[loc] {
        on_all_children_bits(data, data.moves[index].path, &mut |path| callback(path, DropFlagState::Absent));
    }
    let block = &body.basic_blocks[loc.block.0 as usize];
    if loc.statement_index == block.statements.len() {
        if let TerminatorKind::Drop { place, .. } = &block.terminator.as_ref().unwrap().kind {
            on_lookup_result_bits(data, data.rev_lookup.find(place), &mut |path| callback(path, DropFlagState::Absent));
        }
    }
    for_location_inits(data, loc, &mut |path| callback(path, DropFlagState::Present));
}

pub fn for_location_inits(data: &MoveData, loc: Location, callback: &mut dyn FnMut(usize)) {
    for &index in &data.init_loc_map[loc] {
        let init = data.inits[index];
        match init.kind {
            InitKind::Deep => on_all_children_bits(data, init.path, callback),
            InitKind::Shallow => callback(init.path),
            InitKind::NonPanicPathOnly => (),
        }
    }
}

pub(crate) enum InactiveVariants<'a> { Inactives(&'a [VariantIdx]), Active(VariantIdx) }
pub(crate) fn on_all_inactive_variants(data: &MoveData, place: &Place, inactive: InactiveVariants<'_>, callback: &mut dyn FnMut(usize)) {
    let LookupResult::Exact(path) = data.rev_lookup.find(place) else { return; };
    let mut child = data.move_paths[path].first_child;
    while let Some(index) = child {
        let path = &data.move_paths[index];
        let (elem, prefix) = path.place.projection.split_last().unwrap();
        assert_eq!(prefix.len(), place.projection.len());
        let PlaceElem::Downcast(_, variant) = elem else { panic!("enum child must be a downcast"); };
        let is_inactive = match inactive {
            InactiveVariants::Inactives(variants) => variants.contains(variant),
            InactiveVariants::Active(active) => *variant != active,
        };
        if is_inactive { on_all_children_bits(data, index, callback); }
        child = path.next_sibling;
    }
}
