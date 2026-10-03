//! The VM's side of the module census (makepad-script-census): while a
//! census records, each `script_mod` registration is bracketed with
//! [`ScriptVm::census_begin`] / [`ScriptVm::census_end`] under its
//! `module_path!()`, and a Rust type made from a registered default is
//! noted. The census itself (what it records, the heap walk that finds the
//! modules an app used) lives in that crate; the VM only calls it.

use crate::heap::ScriptHeap;
use crate::value::ScriptObject;
use crate::vm::{ScriptCode, ScriptVm};
use std::sync::OnceLock;

/// A recording census, installed in [`ScriptHeap::census`].
pub trait ScriptCensus {
    /// Before a module's `script_mod` registers.
    fn begin(&mut self, heap: &ScriptHeap, code: &ScriptCode, module_path: &'static str);
    /// After it.
    fn end(&mut self, heap: &ScriptHeap, code: &ScriptCode);
    /// A Rust type was made from the registered default `obj`.
    fn default_used(&self, obj: ScriptObject);
    /// The census back, when recording ends.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

/// Called for registrations in a VM that records no census (a document's
/// own VM) while another VM records one: set once by the census.
pub static CENSUS_OTHER_VM: OnceLock<fn(&'static str)> = OnceLock::new();

impl ScriptVm<'_> {
    /// Before a module's `script_mod` registers (a no-op unless recording).
    pub fn census_begin(&mut self, module_path: &'static str) {
        let Some(mut census) = self.bx.heap.census.take() else {
            if let Some(note) = CENSUS_OTHER_VM.get() {
                note(module_path);
            }
            return;
        };
        census.begin(&self.bx.heap, &self.bx.code, module_path);
        self.bx.heap.census = Some(census);
    }

    /// After it.
    pub fn census_end(&mut self) {
        let Some(mut census) = self.bx.heap.census.take() else { return };
        census.end(&self.bx.heap, &self.bx.code);
        self.bx.heap.census = Some(census);
    }
}
