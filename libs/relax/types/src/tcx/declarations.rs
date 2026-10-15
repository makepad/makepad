//! Per-definition declaration queries. Cells are process-local, including after image restore.
use super::{DefId, Program, Sym, Tcx};
use crate::next_solver::{Predicate, TraitRef};
use crate::types::variance::GenericIndices;
use std::sync::{Arc, OnceLock};
use relax_session::sync::Mutex;

#[derive(Default)]
pub(crate) struct DeclarationQueries {
    cells: OnceLock<Vec<OnceLock<Box<DefQueries>>>>,
}

#[derive(Default)]
pub(crate) struct DefQueries {
    krate: u32,
    pub own_clauses: DeclarationCell<Vec<Predicate>>,
    pub super_predicates: DeclarationCell<Vec<Predicate>>,
    filtered_supers: Mutex<crate::fxhash::HashMap<Sym, Arc<DeclarationCell<Vec<Predicate>>>>>,
    pub explicit_item_bounds: DeclarationCell<Vec<Predicate>>,
    pub item_bounds: DeclarationCell<Vec<Predicate>>,
    pub impl_super_outlives: DeclarationCell<Vec<Predicate>>,
    pub generics: DeclarationCell<GenericIndices>,
    pub impl_trait_ref: DeclarationCell<TraitRef>,
}

pub(crate) struct DeclarationCell<T>(OnceLock<Box<Declaration<T>>>);
impl<T> Default for DeclarationCell<T> {
    fn default() -> Self { Self(OnceLock::new()) }
}
struct Declaration<T> {
    value: T,
    misses: Vec<(u32, String)>,
}
impl<T> DeclarationCell<T> {
    pub(crate) fn get_or_init(&self, compute: impl FnOnce() -> T) -> &T {
        let declaration = self.0.get_or_init(|| {
            let enclosing = crate::program::missing_save();
            let value = compute();
            let misses = crate::program::take_missing();
            crate::program::missing_restore(enclosing);
            Box::new(Declaration { value, misses })
        });
        // Replaying nested misses makes every dependent cell retry after a module load.
        if !declaration.misses.is_empty() { crate::program::note_misses(&declaration.misses); }
        &declaration.value
    }
    fn reset_with_misses(&mut self) {
        if self.0.get().is_some_and(|d| !d.misses.is_empty()) { self.0.take(); }
    }
}
impl DefQueries {
    pub(crate) fn filtered_supers(&self, name: Sym, compute: impl FnOnce() -> Vec<Predicate>) -> Vec<Predicate> {
        let cell = self.filtered_supers.lock().unwrap_or_else(|e| e.into_inner())
            .entry(name).or_default().clone();
        cell.get_or_init(compute).clone()
    }

    fn reset_with_misses(&mut self) {
        self.filtered_supers.get_mut().unwrap_or_else(|e| e.into_inner())
            .retain(|_, cell| !cell.0.get().is_some_and(|d| !d.misses.is_empty()));
        self.own_clauses.reset_with_misses();
        self.super_predicates.reset_with_misses();
        self.explicit_item_bounds.reset_with_misses();
        self.item_bounds.reset_with_misses();
        self.impl_super_outlives.reset_with_misses();
        self.generics.reset_with_misses();
        self.impl_trait_ref.reset_with_misses();
    }
}
impl DeclarationQueries {
    pub(super) fn invalidate_visible(&mut self, visible: Option<&[bool]>) {
        if let Some(cells) = self.cells.get_mut() {
            for cell in cells {
                if cell.get().is_some_and(|q| visible.map_or(true, |v| v.get(q.krate as usize).copied().unwrap_or(true))) {
                    cell.take();
                }
            }
        }
    }

    pub(super) fn prepare(&mut self, prog: &Program, loaded: bool, own_crate: bool, visible: Option<&[bool]>) {
        let Some(cells) = self.cells.get_mut() else { return; };
        cells.resize_with(prog.defs.len(), OnceLock::new);
        for (index, cell) in cells.iter_mut().enumerate() {
            if !loaded && !own_crate && visible.map_or(true, |v| v.get(prog.def(DefId(index as u32)).krate as usize).copied().unwrap_or(true)) {
                cell.take();
            } else {
                if let Some(cell) = cell.get_mut() { cell.reset_with_misses(); }
            }
        }
    }
    pub(super) fn invalidate(&mut self, gone: &[DefId]) {
        if let Some(cells) = self.cells.get_mut() {
            for def in gone { if let Some(cell) = cells.get_mut(def.0 as usize) { cell.take(); } }
        }
    }
    pub(super) fn retain_prefix(&mut self, first: usize) {
        if let Some(cells) = self.cells.get_mut() { cells.truncate(first); }
    }
}
impl Tcx {
    pub(crate) fn declaration_queries(&self, prog: &Program, def: DefId) -> &DefQueries {
        // Even a cell hit must publish its declaration dependency to an enclosing query.
        self.queries.read_def_declarations(prog, def).expect("declaration input");
        &self.declarations.cells.get_or_init(|| (0..prog.defs.len()).map(|_| OnceLock::new()).collect())[def.0 as usize]
            .get_or_init(|| Box::new(DefQueries { krate: prog.def(def).krate, ..DefQueries::default() }))
    }
}
