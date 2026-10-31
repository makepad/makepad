//! Runtime-generated wasm modules linked into this module's function
//! table (the browser half of code a host generates at run time, such as
//! compute kernels: `makepad_script_compute::wasm`).
//!
//! A module's bytes go to the page (`FromWasmLinkModule`); it is compiled
//! and instantiated there against this module's memory (`env.memory`), and
//! each exported function is placed in this module's indirect function
//! table (`__indirect_function_table`, exported and growable: the build
//! links with `--export-table --growable-table`). The answer is the table
//! slot of every export, in export order; a slot is called as a function
//! pointer of the export's signature (`call_indirect`, type-checked by the
//! engine), with no JavaScript on the call. `unlink` empties the slots so
//! the instance can be collected.
//!
//! Slots exist in the instance of the thread that linked them (each
//! worker instantiates this module with its own table).

use crate::cx::Cx;
use crate::os::web::from_wasm::{FromWasmLinkModule, FromWasmUnlinkSlots};
use crate::thread::SignalToUI;
use crate::makepad_wasm_bridge::WasmDataU8;
use std::cell::RefCell;
use std::collections::HashMap;

/// A link request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WasmLinkId(pub u32);

thread_local! {
    static NEXT: RefCell<u32> = const { RefCell::new(1) };
    static DONE: RefCell<HashMap<u32, Result<Vec<u32>, String>>> = RefCell::new(HashMap::new());
}

impl Cx {
    /// Asks the page to instantiate `bytes` and link its exported functions
    /// into the function table. The result arrives later (a UI signal is
    /// raised): [`Cx::wasm_link_result`].
    pub fn link_wasm_module(&mut self, bytes: Vec<u8>) -> WasmLinkId {
        let id = NEXT.with(|n| {
            let mut n = n.borrow_mut();
            let id = *n;
            *n = n.wrapping_add(1).max(1);
            id
        });
        self.os.from_wasm(FromWasmLinkModule { request_id: id, bytes: WasmDataU8::from_vec_u8(bytes) });
        WasmLinkId(id)
    }

    /// The table slots of a linked module's exports, in export order, or
    /// why it could not be linked; None while pending (taken once).
    pub fn wasm_link_result(&mut self, id: WasmLinkId) -> Option<Result<Vec<u32>, String>> {
        DONE.with(|d| d.borrow_mut().remove(&id.0))
    }

    /// Empties table slots a link returned (the functions are no longer
    /// called; their instance may be collected).
    pub fn unlink_wasm_slots(&mut self, slots: Vec<u32>) {
        if !slots.is_empty() {
            self.os.from_wasm(FromWasmUnlinkSlots { slots });
        }
    }
}

/// The page answered a link request.
pub(crate) fn linked(request_id: u32, slots: Vec<u32>, error: String) {
    let r = if error.is_empty() { Ok(slots) } else { Err(error) };
    DONE.with(|d| d.borrow_mut().insert(request_id, r));
    SignalToUI::set_ui_signal();
}
